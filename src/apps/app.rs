use super::{
    ApiHandle, AppError, AppsApiClient, Context, ExecutionContext, FunctionHandle, NativeApi,
    NativeFunction, ResourceHandle, ResourceKind, ResourceSpec,
    deploy::{AppDefinition, ResourceDefinition},
    events::EventSink,
    targets::{FunctionDefinition, ResourceAddress, Target},
    tasks::{PodTarget, TaskConfig},
};
use crate::{Client, Error, Result};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex},
};

/// Explicit application routing, connection, and task runtime configuration.
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// Deployed environment used when the invocation context has no override.
    pub environment: String,
    /// Explicit connection pool; absent resolves credentials only when a remote call needs them.
    pub client: Option<Client>,
    /// Fixed routing snapshot; absent reads current environment at each invocation.
    pub context: Option<ExecutionContext>,
    /// Dedicated-pod runtime and runner connection settings.
    pub task: TaskConfig,
}
impl Default for AppConfig {
    fn default() -> Self {
        Self {
            environment: "default".into(),
            client: None,
            context: None,
            task: TaskConfig::default(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct RegisteredFunction {
    pub definition: ResourceDefinition,
    pub function: FunctionDefinition,
    pub body: NativeFunction,
    pub init: Option<NativeFunction>,
}
#[derive(Clone)]
pub(crate) struct RegisteredApi {
    pub definition: ResourceDefinition,
    pub body: NativeApi,
}
enum RegisteredResource {
    Function(RegisteredFunction),
    Api(RegisteredApi),
}
impl RegisteredResource {
    fn definition(&self) -> &ResourceDefinition {
        match self {
            Self::Function(r) => &r.definition,
            Self::Api(r) => &r.definition,
        }
    }
}
#[derive(Default)]
struct AppState {
    resources: Vec<RegisteredResource>,
    dev_targets: BTreeMap<String, Target>,
    events: Option<EventSink>,
}
struct AppInner {
    name: String,
    config: AppConfig,
    state: Mutex<AppState>,
}

/// A named, ordered collection of native application functions and invocation routing state.
///
/// Clones share registrations and development targets. Handles retain that state without
/// reference cycles. Construction and registration never resolve credentials or execute code.
///
/// ```
/// use runpod::{apps::{App, NativeFunction, ResourceSpec, targets::FunctionDefinition}, json};
/// # #[tokio::main]
/// # async fn main() -> runpod::Result<()> {
/// let app = App::new("example")?;
/// let function = FunctionDefinition { name: "echo".into(), parameters: vec!["x".into()], source: None };
/// let echo = app.queue(ResourceSpec::default(), function,
///     NativeFunction::new(|call| async move { Ok(json!(call.args)) }))?;
/// assert_eq!(echo.local(vec![json!(42)], Default::default()).await?, json!([42]));
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct App {
    inner: Arc<AppInner>,
}
impl fmt::Debug for App {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("App")
            .field("name", &self.name())
            .field("environment", &self.environment())
            .finish_non_exhaustive()
    }
}
impl App {
    /// Creates an application with the default environment and lazily resolved credentials.
    ///
    /// # Errors
    /// Returns invalid resource for an empty application name.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        Self::with_config(name, AppConfig::default())
    }

    /// Creates an application with explicit routing and runtime configuration.
    ///
    /// # Errors
    /// Returns invalid resource for an empty application name.
    pub fn with_config(name: impl Into<String>, config: AppConfig) -> Result<Self> {
        let name = name.into();
        if name.is_empty() {
            return Err(
                AppError::InvalidResource("App name must be a non-empty string".into()).into(),
            );
        }
        Ok(Self {
            inner: Arc::new(AppInner {
                name,
                config,
                state: Mutex::default(),
            }),
        })
    }
    /// Returns the application's routing name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.inner.name
    }
    /// Returns the application's default deployment environment.
    #[must_use]
    pub fn environment(&self) -> &str {
        &self.inner.config.environment
    }

    /// Registers a queue function, using its symbol name when the resource name is empty.
    ///
    /// # Errors
    /// Returns invalid configuration, duplicate resource, or poisoned registry errors.
    pub fn queue(
        &self,
        mut spec: ResourceSpec,
        function: FunctionDefinition,
        body: NativeFunction,
    ) -> Result<FunctionHandle> {
        spec.kind = ResourceKind::Queue;
        self.register(spec, function, body)
    }

    /// Registers a dedicated-pod function with the same local invocation interface.
    ///
    /// Remote source-based tasks require explicit [`FunctionDefinition`] source metadata.
    ///
    /// # Errors
    /// Returns invalid configuration, duplicate resource, or poisoned registry errors.
    pub fn task(
        &self,
        mut spec: ResourceSpec,
        function: FunctionDefinition,
        body: NativeFunction,
    ) -> Result<FunctionHandle> {
        spec.kind = ResourceKind::Task;
        self.register(spec, function, body)
    }
    /// Registers named native HTTP routes or explicit metadata for a Python ASGI factory.
    ///
    /// # Errors
    /// Returns invalid resource/route configuration, missing routes, duplicate names, or poisoned state.
    pub fn api(&self, mut spec: ResourceSpec, body: NativeApi) -> Result<ApiHandle> {
        spec.kind = ResourceKind::Api;
        if spec.name.is_empty() {
            body.name().clone_into(&mut spec.name);
        }
        spec.validate()?;
        body.configure_spec(&mut spec)?;
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| AppError::StatePoisoned("registration"))?;
        if state
            .resources
            .iter()
            .any(|r| r.definition().spec.name == spec.name)
        {
            return Err(AppError::InvalidResource(format!(
                "duplicate resource name '{}' in app '{}'",
                spec.name,
                self.name()
            ))
            .into());
        }
        let index = state.resources.len();
        state.resources.push(RegisteredResource::Api(RegisteredApi {
            definition: ResourceDefinition {
                spec,
                symbol: body.symbol().cloned(),
                referenced_secrets: Vec::new(),
            },
            body,
        }));
        Ok(ApiHandle::new(self.clone(), index))
    }
    fn register(
        &self,
        mut spec: ResourceSpec,
        function: FunctionDefinition,
        body: NativeFunction,
    ) -> Result<FunctionHandle> {
        if spec.name.is_empty() {
            spec.name.clone_from(&function.name);
        }
        spec.validate()?;
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| AppError::StatePoisoned("registration"))?;
        if state
            .resources
            .iter()
            .any(|r| r.definition().spec.name == spec.name)
        {
            return Err(AppError::InvalidResource(format!(
                "duplicate resource name '{}' in app '{}'",
                spec.name,
                self.name()
            ))
            .into());
        }
        let index = state.resources.len();
        state
            .resources
            .push(RegisteredResource::Function(RegisteredFunction {
                definition: ResourceDefinition {
                    spec,
                    symbol: None,
                    referenced_secrets: Vec::new(),
                },
                function,
                body,
                init: None,
            }));
        Ok(FunctionHandle::new(self.clone(), index))
    }

    /// Returns a metadata snapshot in registration order without borrowing application locks.
    ///
    /// # Errors
    /// Returns an error if a previous panic poisoned registration state.
    pub fn definition(&self) -> Result<AppDefinition> {
        let state = self
            .inner
            .state
            .lock()
            .map_err(|_| AppError::StatePoisoned("registration"))?;
        Ok(AppDefinition {
            name: self.name().into(),
            environment: self.environment().into(),
            resources: state
                .resources
                .iter()
                .map(|r| r.definition().clone())
                .collect(),
        })
    }

    /// Returns resource handles in registration order; modifying the vector does not mutate the app.
    ///
    /// # Errors
    /// Returns an error if registration state is poisoned.
    pub fn resources(&self) -> Result<Vec<ResourceHandle>> {
        let state = self
            .inner
            .state
            .lock()
            .map_err(|_| AppError::StatePoisoned("registration"))?;
        Ok(state
            .resources
            .iter()
            .enumerate()
            .map(|(index, resource)| match resource {
                RegisteredResource::Function(_) => {
                    ResourceHandle::Function(FunctionHandle::new(self.clone(), index))
                }
                RegisteredResource::Api(_) => {
                    ResourceHandle::Api(ApiHandle::new(self.clone(), index))
                }
            })
            .collect())
    }

    /// Binds a provisioned development target, also usable by sibling calls inside a worker.
    ///
    /// # Errors
    /// Returns an error if routing state is poisoned.
    pub fn set_dev_target(&self, resource: impl Into<String>, target: Target) -> Result<()> {
        let previous = {
            self.inner
                .state
                .lock()
                .map_err(|_| AppError::StatePoisoned("development targets"))?
                .dev_targets
                .insert(resource.into(), target)
        };
        drop(previous);
        Ok(())
    }

    /// Selects observations for subsequent dedicated-pod invocations.
    ///
    /// # Errors
    /// Returns an error if routing state is poisoned.
    pub fn set_events(&self, events: Option<EventSink>) -> Result<()> {
        let previous = {
            let mut state = self
                .inner
                .state
                .lock()
                .map_err(|_| AppError::StatePoisoned("events"))?;
            std::mem::replace(&mut state.events, events)
        };
        drop(previous);
        Ok(())
    }

    pub(crate) fn function(&self, index: usize) -> Result<RegisteredFunction> {
        self.inner
            .state
            .lock()
            .map_err(|_| AppError::StatePoisoned("registration"))?
            .resources
            .get(index)
            .and_then(|r| match r {
                RegisteredResource::Function(r) => Some(r.clone()),
                RegisteredResource::Api(_) => None,
            })
            .ok_or_else(|| {
                Error::InvalidInput("function handle does not belong to this application".into())
            })
    }
    pub(crate) fn update_function<R>(
        &self,
        index: usize,
        update: impl FnOnce(&mut RegisteredFunction) -> R,
    ) -> Result<R> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| AppError::StatePoisoned("registration"))?;
        let function = state
            .resources
            .get_mut(index)
            .and_then(|r| match r {
                RegisteredResource::Function(r) => Some(r),
                RegisteredResource::Api(_) => None,
            })
            .ok_or_else(|| Error::InvalidInput("unknown function handle".into()))?;
        Ok(update(function))
    }
    pub(crate) fn api_resource(&self, index: usize) -> Result<RegisteredApi> {
        self.inner
            .state
            .lock()
            .map_err(|_| AppError::StatePoisoned("registration"))?
            .resources
            .get(index)
            .and_then(|r| match r {
                RegisteredResource::Api(r) => Some(r.clone()),
                RegisteredResource::Function(_) => None,
            })
            .ok_or_else(|| Error::InvalidInput("unknown API handle".into()))
    }
    pub(crate) fn set_api_environment(
        &self,
        index: usize,
        env: &[(String, super::EnvValue)],
    ) -> Result<()> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| AppError::StatePoisoned("registration"))?;
        let Some(RegisteredResource::Api(resource)) = state.resources.get_mut(index) else {
            return Err(Error::InvalidInput("unknown API handle".into()));
        };
        resource.definition.set_environment(env);
        Ok(())
    }
    pub(crate) fn context(&self) -> ExecutionContext {
        self.inner
            .config
            .context
            .clone()
            .unwrap_or_else(ExecutionContext::from_env)
    }
    fn client(&self) -> Result<Client> {
        self.inner
            .config
            .client
            .clone()
            .map_or_else(Client::from_env, Ok)
    }
    fn cached_target(&self, name: &str) -> Result<Option<Target>> {
        Ok(self
            .inner
            .state
            .lock()
            .map_err(|_| AppError::StatePoisoned("development targets"))?
            .dev_targets
            .get(name)
            .cloned())
    }
    fn missing(&self, resource: &str) -> Error {
        AppError::EndpointNotFound {
            app_name: self.name().into(),
            resource_name: resource.into(),
        }
        .into()
    }

    pub(crate) async fn resolve(
        &self,
        function: &RegisteredFunction,
        context: &ExecutionContext,
    ) -> Result<ResolvedTarget> {
        self.resolve_resource(&function.definition.spec, Some(&function.function), context)
            .await
    }
    pub(crate) async fn resolve_resource(
        &self,
        spec: &ResourceSpec,
        function: Option<&FunctionDefinition>,
        context: &ExecutionContext,
    ) -> Result<ResolvedTarget> {
        spec.validate()?;
        if spec.kind == ResourceKind::Task {
            let (specs, events) = {
                let state = self
                    .inner
                    .state
                    .lock()
                    .map_err(|_| AppError::StatePoisoned("registration"))?;
                (
                    state
                        .resources
                        .iter()
                        .map(|r| r.definition().spec.clone())
                        .collect(),
                    state.events.clone(),
                )
            };
            let mut target = PodTarget::with_config(
                self.client()?,
                spec.clone(),
                function
                    .ok_or_else(|| Error::InvalidInput("task requires function metadata".into()))?
                    .clone(),
                self.inner.config.task.clone(),
                Some(specs),
            )?;
            target.set_events(events);
            return Ok(ResolvedTarget::Pod(Box::new(target)));
        }
        if context.kind == Context::Dev {
            return self
                .cached_target(&spec.name)?
                .map(|target| ResolvedTarget::Queue(Box::new(target)))
                .ok_or_else(|| self.missing(&spec.name));
        }
        if context.kind == Context::Worker && context.dev_app.as_deref() == Some(self.name()) {
            if let Some(target) = self.cached_target(&spec.name)? {
                return Ok(ResolvedTarget::Queue(Box::new(target)));
            }
            let client = self.client()?;
            let wanted = dev_endpoint_name(self.name(), &spec.name);
            for endpoint in AppsApiClient::new(client.clone())?
                .list_my_endpoints()
                .await?
            {
                let name = endpoint
                    .get("name")
                    .and_then(crate::Value::as_str)
                    .ok_or_else(|| Error::InvalidResponse("endpoint is missing name".into()))?;
                if name == wanted {
                    let id = endpoint
                        .get("id")
                        .and_then(crate::Value::as_str)
                        .ok_or_else(|| Error::InvalidResponse("endpoint is missing id".into()))?;
                    let target = Target::live(client, id, &spec.name)?;
                    self.set_dev_target(&spec.name, target.clone())?;
                    return Ok(ResolvedTarget::Queue(Box::new(target)));
                }
            }
            return Err(self.missing(&spec.name));
        }
        Ok(ResolvedTarget::Queue(Box::new(Target::sentinel(
            self.client()?,
            ResourceAddress::new(
                self.name(),
                context
                    .environment
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(self.environment()),
                &spec.name,
            ),
        )?)))
    }
}
pub(crate) enum ResolvedTarget {
    Queue(Box<Target>),
    Pod(Box<PodTarget>),
}

/// Caller-owned collection replacing import-time global application registration.
#[derive(Debug, Clone, Default)]
pub struct AppRegistry {
    apps: Vec<App>,
}
impl AppRegistry {
    /// Creates an empty registry without importing modules or running application code.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Records an application in insertion order; identical app names are permitted.
    pub fn register(&mut self, app: App) {
        self.apps.push(app);
    }
    /// Borrows registered applications in insertion order.
    #[must_use]
    pub fn apps(&self) -> &[App] {
        &self.apps
    }
    /// Removes registry membership while existing application handles remain valid.
    pub fn clear(&mut self) {
        self.apps.clear();
    }
    /// Restores an explicit discovery snapshot in its original order.
    pub fn restore(&mut self, apps: Vec<App>) {
        self.apps = apps;
    }
}

/// Produces the upstream stable development endpoint name, disambiguating hyphenated names.
#[must_use]
pub fn dev_endpoint_name(app: &str, resource: &str) -> String {
    let digest = Sha256::digest(format!("{app}/{resource}").as_bytes());
    format!(
        "dev-{app}-{resource}-{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Weak,
        atomic::{AtomicUsize, Ordering},
    };

    struct ReentrantDrop {
        app: Weak<AppInner>,
        count: Arc<AtomicUsize>,
    }
    impl Drop for ReentrantDrop {
        fn drop(&mut self) {
            let app = self.app.upgrade().expect("test keeps application alive");
            assert!(
                app.state.try_lock().is_ok(),
                "user capture dropped while application lock was held"
            );
            self.count.fetch_add(1, Ordering::SeqCst);
        }
    }
    #[test]
    fn replacing_user_callbacks_releases_lock_before_dropping_captures() {
        let app = App::new("test").unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let events = || {
            let probe = ReentrantDrop {
                app: Arc::downgrade(&app.inner),
                count: count.clone(),
            };
            EventSink::new(move |_, _| {
                let _keep = &probe;
                Ok(())
            })
        };
        app.set_events(Some(events())).unwrap();
        app.set_events(None).unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let mut target = Target::live(Client::new("key").unwrap(), "ep", "q").unwrap();
        target.set_events(events()).unwrap();
        app.set_dev_target("q", target).unwrap();
        app.set_dev_target(
            "q",
            Target::live(Client::new("key").unwrap(), "ep", "q").unwrap(),
        )
        .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 2);
        let function = app
            .queue(
                ResourceSpec::default(),
                FunctionDefinition {
                    name: "q".into(),
                    parameters: vec![],
                    source: None,
                },
                NativeFunction::sync(|_| Ok(crate::Value::Null)),
            )
            .unwrap();
        let probe = ReentrantDrop {
            app: Arc::downgrade(&app.inner),
            count: count.clone(),
        };
        function
            .init(NativeFunction::sync(move |_| {
                let _keep = &probe;
                Ok(crate::Value::Null)
            }))
            .unwrap();
        function
            .init(NativeFunction::sync(|_| Ok(crate::Value::Null)))
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }
}
