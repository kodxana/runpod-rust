use super::{
    AppError, AppsApiClient, Context, ExecutionContext, FunctionHandle, NativeFunction,
    ResourceKind, ResourceSpec,
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
#[derive(Default)]
struct AppState {
    functions: Vec<RegisteredFunction>,
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
            .functions
            .iter()
            .any(|r| r.definition.spec.name == spec.name)
        {
            return Err(AppError::InvalidResource(format!(
                "duplicate resource name '{}' in app '{}'",
                spec.name,
                self.name()
            ))
            .into());
        }
        let index = state.functions.len();
        state.functions.push(RegisteredFunction {
            definition: ResourceDefinition {
                spec,
                symbol: None,
                referenced_secrets: Vec::new(),
            },
            function,
            body,
            init: None,
        });
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
                .functions
                .iter()
                .map(|r| r.definition.clone())
                .collect(),
        })
    }

    /// Returns function handles in registration order; modifying the vector does not mutate the app.
    ///
    /// # Errors
    /// Returns an error if registration state is poisoned.
    pub fn resources(&self) -> Result<Vec<FunctionHandle>> {
        let count = self
            .inner
            .state
            .lock()
            .map_err(|_| AppError::StatePoisoned("registration"))?
            .functions
            .len();
        Ok((0..count)
            .map(|index| FunctionHandle::new(self.clone(), index))
            .collect())
    }

    /// Binds a provisioned development target, also usable by sibling calls inside a worker.
    ///
    /// # Errors
    /// Returns an error if routing state is poisoned.
    pub fn set_dev_target(&self, resource: impl Into<String>, target: Target) -> Result<()> {
        self.inner
            .state
            .lock()
            .map_err(|_| AppError::StatePoisoned("development targets"))?
            .dev_targets
            .insert(resource.into(), target);
        Ok(())
    }

    /// Selects observations for subsequent dedicated-pod invocations.
    ///
    /// # Errors
    /// Returns an error if routing state is poisoned.
    pub fn set_events(&self, events: Option<EventSink>) -> Result<()> {
        self.inner
            .state
            .lock()
            .map_err(|_| AppError::StatePoisoned("events"))?
            .events = events;
        Ok(())
    }

    pub(crate) fn function(&self, index: usize) -> Result<RegisteredFunction> {
        self.inner
            .state
            .lock()
            .map_err(|_| AppError::StatePoisoned("registration"))?
            .functions
            .get(index)
            .cloned()
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
            .functions
            .get_mut(index)
            .ok_or_else(|| Error::InvalidInput("unknown function handle".into()))?;
        Ok(update(function))
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
        let spec = &function.definition.spec;
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
                        .functions
                        .iter()
                        .map(|r| r.definition.spec.clone())
                        .collect(),
                    state.events.clone(),
                )
            };
            let mut target = PodTarget::with_config(
                self.client()?,
                spec.clone(),
                function.function.clone(),
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
