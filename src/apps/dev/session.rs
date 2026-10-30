use super::{DevConfig, changed_fields, comparable_payload, endpoint_input, gpu_selection};
use crate::apps::{
    App, AppError, AppsApiClient, ResourceKind, ResourceSpec,
    deploy::AppDefinition,
    events::{EventSink, emit},
    gpu::needs_gpu_catalog,
    targets::Target,
    volume::{VolumeResolver, attach_endpoint_volumes},
};
use crate::{Client, Error, Result, Value, client::required_str, json};
use futures_util::future::BoxFuture;
use std::{collections::BTreeMap, fmt, future::Future};
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
struct EndpointRecord {
    name: String,
    id: String,
    resource: String,
    payload: Option<Value>,
}
struct PendingCreate {
    name: String,
    resource: String,
    task: JoinHandle<Result<Value>>,
}
struct DesiredResource {
    app: App,
    spec: ResourceSpec,
    siblings: Vec<ResourceSpec>,
}
fn desired_resources(apps: &[(App, AppDefinition)]) -> Vec<DesiredResource> {
    let mut desired: Vec<DesiredResource> = Vec::new();
    for (app, definition) in apps {
        for resource in &definition.resources {
            if resource.spec.kind == ResourceKind::Task {
                continue;
            }
            let item = DesiredResource {
                app: app.clone(),
                spec: resource.spec.clone(),
                siblings: definition
                    .resources
                    .iter()
                    .map(|r| r.spec.clone())
                    .collect(),
            };
            desired.push(item);
        }
    }
    desired
}

/// Owns adopted/created endpoints, volume resolution, and development routing for a session.
///
/// `start` and `refresh` keep successful mutations when a later operation fails. `stop`
/// retries only retained endpoints. A canceled create remains owned by this session and is
/// awaited and recorded before cleanup, including when cancellation interrupts `stop`.
/// Use one session per application instance. Fresh app instances may be passed to `refresh`.
///
/// Call [`Self::stop`] before dropping a session; Rust drop cannot await network cleanup.
/// [`Self::run`] cleans up after ordinary completion or returned errors. If its future is
/// canceled, retain the session and await `stop` with the Tokio runtime still running.
#[must_use = "development sessions must be stopped to release their endpoints"]
pub struct DevSession {
    apps: Vec<App>,
    client: Client,
    api: AppsApiClient,
    config: DevConfig,
    generation: u64,
    events: Option<EventSink>,
    endpoints: Vec<EndpointRecord>,
    pending: Option<PendingCreate>,
    resolver: VolumeResolver,
}
impl fmt::Debug for DevSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DevSession")
            .field("apps", &self.apps)
            .field("generation", &self.generation)
            .field("endpoint_ids", &self.endpoint_ids())
            .field("pending_create", &self.pending.is_some())
            .finish_non_exhaustive()
    }
}
impl DevSession {
    /// Captures runtime overrides and uses the same client for control-plane and worker calls.
    ///
    /// # Errors
    /// Returns client initialization errors; construction does not contact Runpod.
    pub fn new(apps: Vec<App>, client: Client) -> Result<Self> {
        let config = DevConfig::from_client(&client);
        Self::with_config(apps, client, config, None)
    }
    /// Creates a session with explicit remote runtime settings and optional observations.
    ///
    /// # Errors
    /// Returns client initialization errors.
    pub fn with_config(
        apps: Vec<App>,
        client: Client,
        config: DevConfig,
        events: Option<EventSink>,
    ) -> Result<Self> {
        let api = AppsApiClient::new(client.clone())?;
        let mut resolver = VolumeResolver::new(api.clone());
        if let Some(events) = &events {
            resolver.set_events(events.clone());
        }
        Ok(Self {
            apps,
            client,
            api,
            config,
            generation: 1,
            events,
            endpoints: Vec::new(),
            pending: None,
            resolver,
        })
    }
    /// Current application instances, replaced on refresh.
    #[must_use]
    pub fn apps(&self) -> &[App] {
        &self.apps
    }
    /// Current worker generation, incremented even when a refresh subsequently fails.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Endpoint IDs retained for cleanup, in ownership order.
    /// An in-flight canceled create is added when a lifecycle operation next settles it.
    #[must_use]
    pub fn endpoint_ids(&self) -> Vec<&str> {
        self.endpoints.iter().map(|r| r.id.as_str()).collect()
    }
    /// Adjusts image/package configuration for subsequent refreshes.
    pub fn config_mut(&mut self) -> &mut DevConfig {
        &mut self.config
    }
    fn emit(&self, event: &str, args: &[Value]) {
        emit(self.events.as_ref(), event, args);
    }
    fn snapshots(&self) -> Result<Vec<(App, AppDefinition)>> {
        self.apps
            .iter()
            .map(|app| Ok((app.clone(), app.definition()?)))
            .collect()
    }
    fn bind_apps(&self) -> Result<()> {
        for app in &self.apps {
            app.bind_development(self.client.clone(), self.events.clone())?;
        }
        Ok(())
    }
    fn record(&mut self, name: &str, resource: &str, id: &str) -> &mut EndpointRecord {
        if let Some(index) = self.endpoints.iter().position(|r| r.name == name) {
            self.endpoints[index].id = id.into();
            self.endpoints[index].resource = resource.into();
            return &mut self.endpoints[index];
        }
        self.endpoints.push(EndpointRecord {
            name: name.into(),
            resource: resource.into(),
            id: id.into(),
            payload: None,
        });
        self.endpoints.last_mut().expect("record just inserted")
    }
    // Await the stored handle by reference: dropping this await leaves ownership in the session.
    async fn settle_create(&mut self) -> Result<Option<Value>> {
        let Some(pending) = self.pending.as_mut() else {
            return Ok(None);
        };
        let result = (&mut pending.task).await;
        let pending = self.pending.take().expect("pending create was awaited");
        let value = result??;
        let id = required_str(&value, "id")?;
        self.record(&pending.name, &pending.resource, id);
        Ok(Some(value))
    }
    async fn settle_canceled_create(&mut self) {
        if let Err(error) = self.settle_create().await {
            tracing::warn!(%error, "canceled dev endpoint create failed; no returned endpoint to track");
        }
    }
    async fn create_tracked(
        &mut self,
        name: &str,
        resource: &str,
        payload: Value,
    ) -> Result<Value> {
        debug_assert!(self.pending.is_none());
        let api = self.api.clone();
        self.pending = Some(PendingCreate {
            name: name.into(),
            resource: resource.into(),
            task: tokio::spawn(async move { api.save_endpoint(&payload).await }),
        });
        self.settle_create()
            .await?
            .ok_or_else(|| Error::InvalidResponse("endpoint create returned no result".into()))
    }
    async fn payload(&mut self, resource: &DesiredResource) -> Result<Value> {
        let spec = &resource.spec;
        spec.validate()?;
        let members = if needs_gpu_catalog(&gpu_selection(spec)?) {
            self.api.gpu_pool_members().await
        } else {
            None
        };
        let mut payload = endpoint_input(
            resource.app.name(),
            spec,
            self.generation,
            &self.config,
            members.as_ref(),
        )?;
        if let Some(id) =
            crate::apps::registry::resolve_registry_auth(spec.registry_auth.as_deref(), &self.api)
                .await?
        {
            payload["template"]["containerRegistryAuthId"] = json!(id);
        }
        if let Some(model) = spec.model.as_ref().filter(|s| !s.is_empty()) {
            payload["modelReferences"] = json!([crate::apps::Model::new(model)?.reference()]);
        }
        attach_endpoint_volumes(&mut payload, spec, &mut self.resolver, &resource.siblings).await?;
        Ok(payload)
    }
    fn bind_target(&self, resource: &DesiredResource, id: &str) -> Result<()> {
        let mut target = Target::live(self.client.clone(), id, &resource.spec.name)?;
        if let Some(events) = &self.events {
            target.set_events(events.clone())?;
        }
        resource.app.set_dev_target(&resource.spec.name, target)
    }
    /// Validates explicit secrets, adopts endpoints by name, and provisions queue/API resources.
    ///
    /// Task resources receive session credentials/events without persistent endpoint creation.
    /// Adopted endpoints are tracked before reconciliation, even if their update fails.
    /// # Errors
    /// Returns configuration, secret, storage, or service errors. Call `stop` after failure.
    pub async fn start(&mut self) -> Result<()> {
        self.settle_canceled_create().await;
        self.emit("session_starting", &[]);
        let snapshots = self.snapshots()?;
        let secrets = snapshots
            .iter()
            .flat_map(|(_, app)| &app.resources)
            .flat_map(|r| r.referenced_secrets.iter().cloned())
            .collect::<Vec<_>>();
        crate::apps::validate_secrets(&secrets, &self.api).await?;
        self.bind_apps()?;
        let existing = self
            .api
            .list_my_endpoints()
            .await?
            .into_iter()
            .map(|e| Ok((required_str(&e, "name")?.to_owned(), e)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        for resource in desired_resources(&snapshots) {
            let spec = &resource.spec;
            let name = crate::apps::dev_endpoint_name(resource.app.name(), &spec.name);
            let existing_id = existing
                .get(&name)
                .map(|e| required_str(e, "id").map(str::to_owned))
                .transpose()?;
            if let Some(id) = &existing_id {
                self.record(&name, &spec.name, id);
            }
            let mut payload = self.payload(&resource).await?;
            let value = if let Some(id) = existing_id {
                self.emit("adopted", &[json!(spec.name), json!(id)]);
                payload["id"] = json!(id);
                self.api.save_endpoint(&payload).await?
            } else {
                self.emit(
                    "provisioning",
                    &[json!(spec.name), json!(spec.kind), json!(hardware(spec)?)],
                );
                self.create_tracked(&name, &spec.name, payload.clone())
                    .await?
            };
            let id = required_str(&value, "id")?;
            self.record(&name, &spec.name, id).payload = Some(comparable_payload(&payload)?);
            self.emit("ready", &[json!(spec.name), json!(id)]);
            self.bind_target(&resource, id)?;
        }
        self.emit("session_started", &[]);
        Ok(())
    }
    /// Deletes removed resources, then updates survivors and creates additions in declaration order.
    ///
    /// Every survivor is saved with a new generation, even when configuration is unchanged.
    /// A removal failure prevents all saves and remains tracked for retry. As upstream,
    /// refresh does not repeat secret preflight and the volume cache persists for the session.
    /// # Errors
    /// Returns cleanup, configuration, storage, or service errors without rolling back mutations.
    pub async fn refresh(&mut self, apps: Vec<App>) -> Result<()> {
        self.settle_canceled_create().await;
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| Error::InvalidInput("development generation overflow".into()))?;
        self.apps = apps;
        self.bind_apps()?;
        let mut desired: Vec<DesiredResource> = Vec::new();
        for item in desired_resources(&self.snapshots()?) {
            if let Some(index) = desired
                .iter()
                .position(|r| r.app.name() == item.app.name() && r.spec.name == item.spec.name)
            {
                desired[index] = item;
            } else {
                desired.push(item);
            }
        }
        let names = desired
            .iter()
            .map(|r| crate::apps::dev_endpoint_name(r.app.name(), &r.spec.name))
            .collect::<Vec<_>>();
        let removed = self
            .endpoints
            .iter()
            .filter(|r| !names.contains(&r.name))
            .cloned()
            .collect::<Vec<_>>();
        let mut failures = Vec::new();
        for record in removed {
            match self.api.delete_endpoint(&record.id).await {
                Ok(_) => {
                    self.endpoints.retain(|r| r.name != record.name);
                    self.emit("resource_removed", &[json!(record.resource)]);
                }
                Err(error) => {
                    failures.push(format!("{} ({}): {error}", record.resource, record.id));
                    self.emit("delete_failed", &[json!(record.resource)]);
                }
            }
        }
        if !failures.is_empty() {
            return Err(AppError::DevCleanup(failures).into());
        }
        for (name, resource) in names.into_iter().zip(desired) {
            let mut payload = self.payload(&resource).await?;
            let comparable = comparable_payload(&payload)?;
            let previous = self.endpoints.iter().find(|r| r.name == name).cloned();
            let value = if let Some(previous) = &previous {
                payload["id"] = json!(previous.id);
                self.api.save_endpoint(&payload).await?
            } else {
                self.create_tracked(&name, &resource.spec.name, payload)
                    .await?
            };
            let id = required_str(&value, "id")?;
            self.record(&name, &resource.spec.name, id).payload = Some(comparable.clone());
            self.bind_target(&resource, id)?;
            if let Some(previous) = previous.and_then(|r| r.payload) {
                if !super::equivalent(&previous, &comparable) {
                    let fields = changed_fields(&previous, &comparable)?;
                    self.emit(
                        "resource_changed",
                        &[json!(resource.spec.name), json!(fields)],
                    );
                }
            } else {
                self.emit(
                    "resource_added",
                    &[
                        json!(resource.spec.name),
                        json!(resource.spec.kind),
                        json!(hardware(&resource.spec)?),
                    ],
                );
            }
        }
        Ok(())
    }
    /// Settles any canceled create, deletes owned endpoints, and clears current app bindings.
    ///
    /// Failed IDs stay tracked and a subsequent call retries only those failures.
    /// # Errors
    /// Returns an aggregate cleanup error or poisoned application-state error.
    pub async fn stop(&mut self) -> Result<()> {
        self.stop_with_events(None).await
    }
    /// Uses an override event sink for teardown, otherwise the session's sink.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::stop`].
    pub async fn stop_with_events(&mut self, events: Option<&EventSink>) -> Result<()> {
        self.settle_canceled_create().await;
        let sink = events.or(self.events.as_ref());
        let pending = self.endpoints.clone();
        let mut failures = Vec::new();
        emit(sink, "cleanup_started", &[json!(pending.len())]);
        for record in pending {
            emit(sink, "deleting", &[json!(record.resource)]);
            match self.api.delete_endpoint(&record.id).await {
                Ok(_) => {
                    self.endpoints.retain(|r| r.name != record.name);
                    emit(sink, "deleted", &[json!(record.resource)]);
                }
                Err(error) => {
                    failures.push(format!("{} ({}): {error}", record.resource, record.id));
                    emit(sink, "delete_failed", &[json!(record.resource)]);
                }
            }
        }
        for app in &self.apps {
            app.clear_development()?;
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(AppError::DevCleanup(failures).into())
        }
    }
    /// Runs application work with startup/error cleanup, replacing Python's async context manager.
    ///
    /// Cleanup failure takes precedence over a body/start error, matching upstream behavior.
    /// On future cancellation or panic, explicitly call `stop` on the retained session.
    /// # Errors
    /// Returns startup, body, or cleanup errors.
    pub async fn run<T, F>(&mut self, work: F) -> Result<T>
    where
        F: for<'a> FnOnce(&'a mut Self) -> BoxFuture<'a, Result<T>>,
    {
        let result = match self.start().await {
            Ok(()) => work(self).await,
            Err(error) => Err(error),
        };
        self.stop().await?;
        result
    }

    /// Runs until work completes or the supplied shutdown future resolves, then awaits cleanup.
    ///
    /// Returns `None` on shutdown, including during startup. This provides deterministic cleanup
    /// for interrupt signals without canceling the session's owner. Keep awaiting this method
    /// until it returns; dropping its future still requires an explicit subsequent `stop`.
    /// # Errors
    /// Returns startup, body, or cleanup errors. Cleanup failure takes precedence.
    pub async fn run_until<T, F, S>(&mut self, work: F, shutdown: S) -> Result<Option<T>>
    where
        F: for<'a> FnOnce(&'a mut Self) -> BoxFuture<'a, Result<T>>,
        S: Future<Output = ()>,
    {
        let result = tokio::select! {
            () = shutdown => Ok(None),
            result = async { self.start().await?; work(self).await } => result.map(Some),
        };
        self.stop().await?;
        result
    }
}

fn hardware(spec: &ResourceSpec) -> Result<String> {
    let selection = match &spec.cpu {
        Some(cpu) => cpu.values().to_vec(),
        None => gpu_selection(spec)?,
    };
    Ok(if selection.is_empty() {
        "any".into()
    } else {
        selection.join(",")
    })
}
