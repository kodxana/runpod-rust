use super::{
    App, AppError, FunctionArgs, NativeFunction, ResourceSpec, RouteSpec, app::ResolvedTarget,
    deploy::SymbolReference, serialization::FunctionSource, targets::DEFAULT_TIMEOUT,
};
use crate::{Error, Result, Value};
use std::{collections::BTreeMap, fmt, time::Duration};

#[derive(Clone)]
enum ApiBody {
    Routes {
        methods: BTreeMap<String, (RouteSpec, NativeFunction)>,
        init: Option<(String, NativeFunction)>,
    },
    AsgiFactory(String),
}

/// Native route declarations and lifecycle hook, or explicit interoperable ASGI factory metadata.
///
/// Route handlers own their Rust state through closures. Registration does not instantiate
/// Python classes or execute a factory. ASGI factory metadata is for existing Python workers.
#[derive(Clone)]
pub struct NativeApi {
    name: String,
    source: Option<FunctionSource>,
    symbol: Option<SymbolReference>,
    body: ApiBody,
}
impl fmt::Debug for NativeApi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeApi")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}
impl NativeApi {
    /// Creates a named declaration; at least one route must be added before app registration.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source: None,
            symbol: None,
            body: ApiBody::Routes {
                methods: BTreeMap::new(),
                init: None,
            },
        }
    }
    /// Describes an existing Python ASGI factory without importing or invoking Python locally.
    #[must_use]
    pub fn asgi_factory(name: impl Into<String>, qualname: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source: None,
            symbol: None,
            body: ApiBody::AsgiFactory(qualname.into()),
        }
    }
    /// Returns the default resource name supplied by this declaration.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Adds a native method, rejecting reserved paths and duplicate verb/path pairs.
    ///
    /// Replacing the same handler name mirrors replacing a Python class member. Metadata
    /// is emitted in handler-name order, matching class member inspection.
    ///
    /// # Errors
    /// Returns invalid method/path, duplicate route, or incompatible factory declaration errors.
    pub fn route(&mut self, route: RouteSpec, handler: NativeFunction) -> Result<()> {
        super::spec::validate_route(&route.method, &route.path)?;
        let ApiBody::Routes { methods, .. } = &mut self.body else {
            return Err(
                AppError::InvalidResource("ASGI factories define their own routes".into()).into(),
            );
        };
        if let Some((other, _)) = methods.iter().find(|(name, (existing, _))| {
            *name != &route.handler_name
                && existing.method == route.method
                && existing.path == route.path
        }) {
            return Err(AppError::InvalidResource(format!(
                "duplicate route {} {} on {}: '{}' and '{}'",
                route.method, route.path, self.name, other, route.handler_name
            ))
            .into());
        }
        methods.insert(route.handler_name.clone(), (route, handler));
        Ok(())
    }

    /// Registers one named startup hook; replacing that same name is allowed.
    ///
    /// # Errors
    /// Rejects multiple differently named hooks or an ASGI factory's externally owned lifecycle.
    pub fn init(&mut self, name: impl Into<String>, hook: NativeFunction) -> Result<()> {
        let name = name.into();
        let ApiBody::Routes { init, .. } = &mut self.body else {
            return Err(AppError::InvalidResource(
                "ASGI factories define their own initialization".into(),
            )
            .into());
        };
        if let Some((previous, _)) = init
            && previous != &name
        {
            return Err(AppError::InvalidResource(format!(
                "multiple @init methods on {}: '{}' and '{}'",
                self.name, previous, name
            ))
            .into());
        }
        *init = Some((name, hook));
        Ok(())
    }
    /// Attaches complete source text or a re-readable source file for live interoperability.
    pub fn set_source(&mut self, source: FunctionSource) {
        self.source = Some(source);
    }
    /// Attaches explicit module/qualified-name metadata for deployment manifests.
    pub fn set_symbol(&mut self, symbol: SymbolReference) {
        self.symbol = Some(symbol);
    }
    pub(crate) fn symbol(&self) -> Option<&SymbolReference> {
        self.symbol.as_ref()
    }
    pub(crate) fn configure_spec(&self, spec: &mut ResourceSpec) -> Result<()> {
        match &self.body {
            ApiBody::Routes { methods, .. } => {
                if methods.is_empty() {
                    return Err(AppError::InvalidResource(format!("@app.api class {} defines no routes; mark methods with @get/@post/@put/@delete/@patch", self.name)).into());
                }
                spec.routes = methods.values().map(|(route, _)| route.clone()).collect();
                spec.asgi_factory = None;
            }
            ApiBody::AsgiFactory(qualname) => {
                spec.routes.clear();
                spec.asgi_factory = Some(qualname.clone());
            }
        }
        Ok(())
    }
    pub(super) fn http_routes(&self) -> Result<Vec<RouteSpec>> {
        match &self.body {
            ApiBody::Routes { methods, .. } => {
                Ok(methods.values().map(|(route, _)| route.clone()).collect())
            }
            ApiBody::AsgiFactory(_) => Err(AppError::InvalidResource(
                "ASGI factories execute in a Python worker".into(),
            )
            .into()),
        }
    }
    pub(super) async fn initialize(&self) -> Result<()> {
        match &self.body {
            ApiBody::Routes { init, .. } => {
                if let Some((_, hook)) = init {
                    hook.call(FunctionArgs::default()).await?;
                }
                Ok(())
            }
            ApiBody::AsgiFactory(_) => Err(AppError::InvalidResource(
                "ASGI factories execute in a Python worker".into(),
            )
            .into()),
        }
    }
    pub(super) async fn call_handler(&self, name: &str, args: FunctionArgs) -> Result<Value> {
        let ApiBody::Routes { methods, .. } = &self.body else {
            return Err(AppError::InvalidResource(
                "ASGI factories execute in a Python worker".into(),
            )
            .into());
        };
        let (_, handler) = methods.get(name).ok_or_else(|| {
            Error::InvalidInput(format!("unknown API handler '{name}' on {}", self.name))
        })?;
        handler.call(args).await
    }
}

/// HTTP client and native handler access for one registered application API.
#[derive(Debug, Clone)]
pub struct ApiHandle {
    app: App,
    index: usize,
}
impl ApiHandle {
    pub(crate) fn new(app: App, index: usize) -> Self {
        Self { app, index }
    }
    /// Returns a snapshot of resource and route metadata in handler-name order.
    ///
    /// # Errors
    /// Returns poisoned registration errors.
    pub fn spec(&self) -> Result<ResourceSpec> {
        Ok(self.app.api_resource(self.index)?.definition.spec)
    }
    /// Retains typed secret references while rendering worker environment values.
    ///
    /// # Errors
    /// Returns poisoned registration errors.
    pub fn set_environment(&self, env: &[(String, super::EnvValue)]) -> Result<()> {
        self.app.set_api_environment(self.index, env)
    }
    /// Runs the native startup hook explicitly for direct method execution.
    /// [`Self::serve`] runs initialization itself before accepting requests.
    ///
    /// # Errors
    /// Returns hook/registration errors or rejects a Python ASGI factory.
    pub async fn initialize(&self) -> Result<()> {
        self.app.api_resource(self.index)?.body.initialize().await
    }

    /// Serve this resource's native routes after its initialization hook completes.
    ///
    /// Matches the Apps runtime's class adapter: GET calls have no arguments; other verbs
    /// pass one positional argument for valid non-null JSON, or no arguments otherwise.
    /// Query/path parameters are not forwarded. `/ping` reports healthy after initialization.
    /// Shutdown stops acceptance and drains active requests. Python ASGI factory metadata
    /// cannot execute in this native server. Live source loading is a separate runtime contract.
    ///
    /// # Errors
    /// Returns invalid route patterns, initialization, registration, or listener errors.
    pub async fn serve(
        &self,
        listener: tokio::net::TcpListener,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Result<()> {
        let resource = self.app.api_resource(self.index)?;
        super::http::serve(
            resource.definition.spec.name,
            resource.body,
            listener,
            shutdown,
        )
        .await
    }

    /// Bind a native Apps HTTP worker and run until Ctrl-C or, on Unix, SIGTERM.
    /// Use the platform's `PORT` value (normally 80) when binding a deployed API worker.
    ///
    /// # Errors
    /// Returns bind, route, initialization, registration, or signal-registration errors.
    pub async fn start(&self, host: &str, port: u16) -> Result<()> {
        let listener = tokio::net::TcpListener::bind((host, port)).await?;
        #[cfg(unix)]
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        self.serve(listener, async move {
            #[cfg(unix)]
            tokio::select! {
                _ = terminate.recv() => {},
                result = tokio::signal::ctrl_c() => {if let Err(error) = result {tracing::warn!(%error,"Ctrl-C listener failed");}}
            }
            #[cfg(not(unix))]
            if let Err(error) = tokio::signal::ctrl_c().await {tracing::warn!(%error,"Ctrl-C listener failed");}
        }).await
    }
    /// Calls a native method by its declared handler name without making an HTTP request.
    ///
    /// The worker/router supplies decoded arguments; no route matching or Python binding occurs here.
    ///
    /// # Errors
    /// Returns missing handler, user function, registration, or incompatible factory errors.
    pub async fn call_handler(&self, name: &str, args: FunctionArgs) -> Result<Value> {
        self.app
            .api_resource(self.index)?
            .body
            .call_handler(name, args)
            .await
    }

    /// Resolves the resource and sends an HTTP call, synchronizing explicit live source first.
    ///
    /// Native routes are never executed locally by this client method, even in their own worker.
    ///
    /// # Errors
    /// Returns discovery, validation, source synchronization, routing, or HTTP errors.
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<Value> {
        let resource = self.app.api_resource(self.index)?;
        let spec = &resource.definition.spec;
        let context = self.app.context();
        context.guard_discovery(&spec.name)?;
        let ResolvedTarget::Queue(mut target) =
            self.app.resolve_resource(spec, None, &context).await?
        else {
            return Err(AppError::InvalidResource(
                "API resources cannot use a dedicated task target".into(),
            )
            .into());
        };
        if target.is_live()
            && let Some(source) = resource.body.source
        {
            target.attach_source(source, &spec.name, Some(spec.clone()))?;
        }
        target.request(method, path, body, timeout).await
    }
    /// Sends GET using the default request timeout.
    ///
    /// # Errors
    /// Returns discovery, routing, synchronization, or HTTP errors.
    pub async fn get(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.request("GET", path, body, DEFAULT_TIMEOUT).await
    }
    /// Sends POST using the default request timeout.
    ///
    /// # Errors
    /// Returns discovery, routing, synchronization, or HTTP errors.
    pub async fn post(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.request("POST", path, body, DEFAULT_TIMEOUT).await
    }
    /// Sends PUT using the default request timeout.
    ///
    /// # Errors
    /// Returns discovery, routing, synchronization, or HTTP errors.
    pub async fn put(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.request("PUT", path, body, DEFAULT_TIMEOUT).await
    }
    /// Sends DELETE using the default request timeout.
    ///
    /// # Errors
    /// Returns discovery, routing, synchronization, or HTTP errors.
    pub async fn delete(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.request("DELETE", path, body, DEFAULT_TIMEOUT).await
    }
    /// Sends PATCH using the default request timeout.
    ///
    /// # Errors
    /// Returns discovery, routing, synchronization, or HTTP errors.
    pub async fn patch(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.request("PATCH", path, body, DEFAULT_TIMEOUT).await
    }
}
