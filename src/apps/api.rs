//! Control-plane operations used by application provisioning and deployment.
use super::{
    endpoint_input::rest_endpoint_input,
    gpu::{GpuCatalog, PoolMembers},
    graphql,
    volume::ComputeType,
};
use crate::{
    Client, Error, Result, Value,
    client::{required_str, segment, truthy},
    control::cpu_config,
    json,
};
use std::{collections::BTreeSet, path::Path, sync::Arc, time::Duration};
use tokio::io::AsyncReadExt;

/// Product whose available capacity is being queried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Product {
    /// Dedicated pods in the secure cloud.
    Pod,
    /// Managed serverless workers.
    Serverless,
}
impl Product {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pod => "POD",
            Self::Serverless => "SERVERLESS",
        }
    }
}

/// An Apps connection with isolated anonymous authentication and upload requests.
#[derive(Debug, Clone)]
pub struct AppsApiClient {
    client: Client,
    anonymous: Client,
    catalog: GpuCatalog,
}

impl AppsApiClient {
    /// Uses an existing client's credentials and control-plane service address.
    ///
    /// # Errors
    /// Returns TLS initialization or service URL errors.
    pub fn new(client: Client) -> Result<Self> {
        let anonymous = Client::anonymous(client.config.api_base_url.clone())?;
        let catalog = GpuCatalog::new(client.clone());
        Ok(Self {
            client,
            anonymous,
            catalog,
        })
    }

    /// Creates a client for browser login before credentials are available.
    ///
    /// # Errors
    /// Returns TLS initialization or service URL errors.
    pub fn anonymous(api_base_url: impl Into<String>) -> Result<Self> {
        Self::new(Client::anonymous(api_base_url)?)
    }

    /// Reads cached GPU pool membership, returning None for static fallback on failure.
    pub async fn gpu_pool_members(&self) -> Option<PoolMembers> {
        self.catalog.live_pool_members().await
    }

    async fn execute(
        &self,
        query: &str,
        variables: Option<&Value>,
        anonymous: bool,
        retry: bool,
    ) -> Result<Value> {
        let client = if anonymous {
            &self.anonymous
        } else {
            &self.client
        };
        let attempts = if retry { 4 } else { 1 };
        for attempt in 0..attempts {
            match client
                .graphql_query(query, variables, Duration::from_secs(60))
                .await
            {
                Err(error)
                    if attempt + 1 < attempts
                        && matches!(error, Error::Http(_) | Error::Io(_) | Error::Timeout) =>
                {
                    tokio::time::sleep(Duration::from_secs(1 << attempt)).await
                }
                result => return field(result?, "data"),
            }
        }
        unreachable!("at least one GraphQL attempt always returns")
    }

    async fn mutation(&self, query: &str, variables: &Value, result: &str) -> Result<Value> {
        field(
            self.execute(query, Some(variables), false, false).await?,
            result,
        )
    }

    /// Saves an endpoint atomically, selecting GraphQL for settings REST cannot preserve.
    ///
    /// # Errors
    /// Returns invalid input, empty read, transport, or API errors; mutations are never retried.
    pub async fn save_endpoint(&self, input: &Value) -> Result<Value> {
        let mut body = rest_endpoint_input(input)?;
        let id = input
            .get("id")
            .filter(|v| truthy(v))
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| Error::InvalidInput("endpoint id must be a string".into()))
            })
            .transpose()?;
        if let (Some(body_value), Some(id)) = (&mut body, id) {
            let path = format!("/v2/serverless/{}", segment(id));
            let current = self
                .client
                .run_rest_request("GET", &path, None, &[])
                .await?;
            if !truthy(&current) {
                return Err(query_error(
                    format!("endpoint '{id}' returned an empty response"),
                    format!("GET {path}"),
                ));
            }
            let expected = input
                .get("type")
                .filter(|v| !v.is_null())
                .map_or(current["type"].as_str(), |v| {
                    Some(if v == "LB" { "LOAD_BALANCER" } else { "QUEUE" })
                });
            if current["type"].as_str() != expected
                || (body_value.get("cpu").is_some() && !truthy(&current["cpu"]))
                || (body_value.get("gpu").is_some() && truthy(&current["cpu"]))
            {
                body = None;
            } else if current["type"] == "QUEUE"
                && body_value.get("scaling").unwrap_or(&current["scaling"])["type"]
                    == "REQUEST_COUNT"
                && let Some(workers) = body_value.get_mut("workers").and_then(Value::as_object_mut)
            {
                workers.remove("idleTimeout");
            }
        }
        if let Some(body) = body {
            let path = id.map_or_else(
                || "/v2/serverless".into(),
                |id| format!("/v2/serverless/{}", segment(id)),
            );
            return self
                .client
                .run_rest_request(
                    if id.is_some() { "PATCH" } else { "POST" },
                    &path,
                    Some(&body),
                    &[],
                )
                .await;
        }
        let mut input = input.clone();
        if let Some(name) = input.get("name").cloned()
            && let Some(template) = input.get_mut("template").and_then(Value::as_object_mut)
            && !template.contains_key("name")
        {
            template.insert("name".into(), name);
        }
        self.mutation(
            graphql::MUTATION_SAVE_ENDPOINT,
            &json!({"input":input}),
            "saveEndpoint",
        )
        .await
    }

    /// Deletes an endpoint and confirms that the request succeeded.
    ///
    /// # Errors
    /// Returns request errors without reporting a failed deletion as success.
    pub async fn delete_endpoint(&self, id: &str) -> Result<bool> {
        self.client
            .run_rest_request(
                "DELETE",
                &format!("/v2/serverless/{}", segment(id)),
                None,
                &[],
            )
            .await?;
        Ok(true)
    }

    /// Lists the current account's endpoints.
    ///
    /// # Errors
    /// Returns request or invalid response errors.
    pub async fn list_my_endpoints(&self) -> Result<Vec<Value>> {
        list(field(
            self.client
                .run_rest_request("GET", "/v2/serverless", None, &[])
                .await?,
            "endpoints",
        )?)
    }

    /// Reads worker state using the user key and a ten-second request timeout.
    ///
    /// # Errors
    /// Returns request errors.
    pub async fn endpoint_workers(&self, id: &str) -> Result<Value> {
        self.client
            .request(
                &self.client.config.api_base_url,
                "GET",
                &format!("/v2/serverless/{}/workers", segment(id)),
                None,
                &[],
                Duration::from_secs(10),
            )
            .await
    }

    /// Provisions a task pod, trying CPU alternatives only after definite capacity errors.
    ///
    /// # Errors
    /// Rejects empty CPU selections; ambiguous provisioning failures propagate without retry.
    pub async fn deploy_task_pod(&self, input: &Value, compute: ComputeType) -> Result<Value> {
        let mut input = input
            .as_object()
            .cloned()
            .ok_or_else(|| Error::InvalidInput("pod input must be an object".into()))?;
        if compute == ComputeType::Cpu {
            let instances = input
                .remove("instanceIds")
                .filter(|v| !v.is_null())
                .unwrap_or_else(|| json!([input.remove("instanceId").unwrap_or(Value::Null)]));
            let instances = instances
                .as_array()
                .filter(|v| {
                    !v.is_empty()
                        && v.iter()
                            .all(|id| id.as_str().is_some_and(|s| !s.trim().is_empty()))
                })
                .ok_or_else(|| {
                    Error::InvalidInput("cpu tasks require non-empty instance ids".into())
                })?;
            for (index, id) in instances.iter().enumerate() {
                input.insert("instanceId".into(), id.clone());
                match self
                    .mutation(
                        graphql::MUTATION_DEPLOY_CPU_POD,
                        &json!({"input":input}),
                        "deployCpuPod",
                    )
                    .await
                {
                    Err(error @ Error::Query { .. })
                        if index + 1 < instances.len() && is_capacity_error(&error) =>
                    {
                        continue;
                    }
                    result => return result,
                }
            }
            unreachable!("nonempty CPU selection always produces a result")
        }
        self.mutation(
            graphql::MUTATION_DEPLOY_POD,
            &json!({"input":input}),
            "podFindAndDeployOnDemand",
        )
        .await
    }

    /// Terminates a task pod.
    ///
    /// # Errors
    /// Returns request errors.
    pub async fn terminate_pod(&self, id: &str) -> Result<()> {
        self.client.terminate_pod(id).await
    }

    /// Looks up an application, returning None for the backend's not-found response.
    ///
    /// # Errors
    /// Returns request or response errors other than not-found.
    pub async fn get_app_by_name(&self, name: &str) -> Result<Option<Value>> {
        match self
            .execute(
                graphql::QUERY_FLASH_APP_BY_NAME,
                Some(&json!({"flashAppName":name})),
                false,
                true,
            )
            .await
        {
            Err(Error::Query { message, .. }) if message.to_lowercase().contains("not found") => {
                Ok(None)
            }
            result => optional_field(result?, "flashAppByName"),
        }
    }

    /// Creates an application without automatically replaying the mutation.
    ///
    /// # Errors
    /// Returns request or response errors.
    pub async fn create_app(&self, name: &str) -> Result<Value> {
        self.mutation(
            graphql::MUTATION_CREATE_FLASH_APP,
            &json!({"input":{"name":name}}),
            "createFlashApp",
        )
        .await
    }

    /// Creates a named environment within an application.
    ///
    /// # Errors
    /// Returns request or response errors.
    pub async fn create_environment(&self, app_id: &str, name: &str) -> Result<Value> {
        self.mutation(
            graphql::MUTATION_CREATE_FLASH_ENVIRONMENT,
            &json!({"input":{"flashAppId":app_id,"name":name}}),
            "createFlashEnvironment",
        )
        .await
    }

    /// Reads GPU availability for a count, datacenter, and product in the secure cloud.
    ///
    /// # Errors
    /// Returns request or response errors.
    pub async fn gpu_stock_status(
        &self,
        gpu_id: &str,
        data_center_id: &str,
        count: u64,
        product: Product,
    ) -> Result<Option<String>> {
        let count = count.to_string();
        let data = self
            .client
            .run_rest_request(
                "GET",
                &format!("/v2/catalog/gpus/{}", segment(gpu_id)),
                None,
                &[
                    ("include", "AVAILABILITY"),
                    ("product", product.as_str()),
                    ("count", &count),
                    ("cloud", "SECURE"),
                ],
            )
            .await?;
        stock_in_datacenter(&data, data_center_id)
    }

    /// Reads CPU stock, using GraphQL for sizes unsupported by the REST catalog.
    ///
    /// # Errors
    /// Returns invalid instance input, request, or response errors.
    pub async fn cpu_stock_status(
        &self,
        instance_id: &str,
        data_center_id: &str,
        product: Product,
    ) -> Result<Option<String>> {
        let cpu = cpu_config(instance_id)?;
        let count = cpu["vcpuCount"]
            .as_u64()
            .expect("cpu_config produces a u64 count");
        if count < 2 || !count.is_power_of_two() {
            let is_sls = product == Product::Serverless;
            let data = self.execute(graphql::QUERY_CPU_STOCK,Some(&json!({"cpuFlavorInput":{"id":cpu["id"],"isSls":is_sls},"specificsInput":{"dataCenterId":data_center_id,"instanceId":instance_id,"isSls":is_sls}})),false,true).await?;
            return optional_string(&data["cpuFlavors"][0]["specifics"]["stockStatus"]);
        }
        let count = count.to_string();
        let data = self
            .client
            .run_rest_request(
                "GET",
                &format!("/v2/catalog/cpus/{}", segment(required_str(&cpu, "id")?)),
                None,
                &[
                    ("include", "AVAILABILITY"),
                    ("product", product.as_str()),
                    ("vcpuCount", &count),
                ],
            )
            .await?;
        stock_in_datacenter(&data, data_center_id)
    }

    /// Finds datacenters supporting at least one network volume tier.
    ///
    /// # Errors
    /// Rejects malformed catalogs rather than provisioning with incomplete placement data.
    pub async fn network_volume_datacenters(&self) -> Result<BTreeSet<String>> {
        let data = self
            .client
            .run_rest_request("GET", "/v2/catalog/datacenters", None, &[])
            .await?;
        let datacenters = data
            .get("dataCenters")
            .and_then(Value::as_array)
            .ok_or_else(|| query_error("datacenter catalog is missing dataCenters", ""))?;
        let mut supported = BTreeSet::new();
        for dc in datacenters {
            let id = dc
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            let tiers = dc
                .get("networkVolumeTypes")
                .and_then(Value::as_array)
                .filter(|a| a.iter().all(|v| v.as_str().is_some_and(|s| !s.is_empty())));
            let (Some(id), Some(tiers)) = (id, tiers) else {
                return Err(query_error(
                    "datacenter catalog has invalid networkVolumeTypes",
                    "",
                ));
            };
            if !tiers.is_empty() {
                supported.insert(id.to_owned());
            }
        }
        Ok(supported)
    }

    /// Collects every page of global volumes.
    ///
    /// # Errors
    /// Returns request, malformed pagination, or exhausted transport retry errors.
    pub async fn list_global_volumes(&self) -> Result<Vec<Value>> {
        self.client
            .run_rest_paginated_request("/v2/global-volumes", "globalVolumes")
            .await
    }

    /// Creates a global volume once and requires a usable identifier in the response.
    ///
    /// # Errors
    /// An invalid response may follow a successful creation; callers must inspect existing volumes.
    pub async fn create_global_volume(&self, name: &str) -> Result<Value> {
        let data = self
            .client
            .run_rest_request(
                "POST",
                "/v2/global-volumes",
                Some(&json!({"name":name})),
                &[],
            )
            .await?;
        if !data
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
        {
            return Err(query_error(
                "global volume creation returned an invalid response; the volume may have been created, check existing volumes before retrying",
                "POST /v2/global-volumes",
            ));
        }
        Ok(data)
    }

    /// Collects every page of network volumes.
    ///
    /// # Errors
    /// Returns request, malformed pagination, or exhausted transport retry errors.
    pub async fn list_network_volumes(&self) -> Result<Vec<Value>> {
        self.client
            .run_rest_paginated_request("/v2/network-volumes", "networkVolumes")
            .await
    }

    /// Creates a network volume of the requested size in gigabytes.
    ///
    /// # Errors
    /// Returns request errors; creation is not retried.
    pub async fn create_network_volume(
        &self,
        name: &str,
        size: u64,
        data_center_id: &str,
    ) -> Result<Value> {
        self.client
            .run_rest_request(
                "POST",
                "/v2/network-volumes",
                Some(&json!({"name":name,"size":size,"dataCenter":data_center_id})),
                &[],
            )
            .await
    }

    /// Lists registry credentials without modifying them.
    ///
    /// # Errors
    /// Returns request or response errors.
    pub async fn list_registry_auths(&self) -> Result<Vec<Value>> {
        list(field(
            self.client
                .run_rest_request("GET", "/v2/registries", None, &[])
                .await?,
            "registries",
        )?)
    }

    /// Stores credentials for a container registry.
    ///
    /// # Errors
    /// Returns request errors; creation is not retried.
    pub async fn create_registry_auth(
        &self,
        name: &str,
        username: &str,
        password: &str,
    ) -> Result<Value> {
        self.client
            .run_rest_request(
                "POST",
                "/v2/registries",
                Some(&json!({"name":name,"username":username,"password":password})),
                &[],
            )
            .await
    }

    /// Deletes registry credentials and confirms request success.
    ///
    /// # Errors
    /// Returns request errors.
    pub async fn delete_registry_auth(&self, id: &str) -> Result<bool> {
        self.client
            .run_rest_request(
                "DELETE",
                &format!("/v2/registries/{}", segment(id)),
                None,
                &[],
            )
            .await?;
        Ok(true)
    }

    /// Lists account secrets, treating an absent collection as empty.
    ///
    /// # Errors
    /// Returns request or response errors.
    pub async fn list_secrets(&self) -> Result<Vec<Value>> {
        let data = self
            .execute(graphql::QUERY_SECRETS, None, false, true)
            .await?;
        list_or_empty(&data["myself"]["secrets"])
    }

    /// Creates a named secret with a description.
    ///
    /// # Errors
    /// Returns request or response errors; creation is not retried.
    pub async fn create_secret(&self, name: &str, value: &str, description: &str) -> Result<Value> {
        self.mutation(
            graphql::MUTATION_CREATE_SECRET,
            &json!({"input":{"name":name,"value":value,"description":description}}),
            "secretCreate",
        )
        .await
    }

    /// Deletes a secret and returns the backend confirmation.
    ///
    /// # Errors
    /// Returns request or response errors.
    pub async fn delete_secret(&self, id: &str) -> Result<bool> {
        let data = self
            .execute(
                graphql::MUTATION_DELETE_SECRET,
                Some(&json!({"id":id})),
                false,
                false,
            )
            .await?;
        Ok(truthy(&data["secretDelete"]))
    }

    /// Lists applications with their environments and builds.
    ///
    /// # Errors
    /// Returns request or response errors.
    pub async fn list_apps(&self) -> Result<Vec<Value>> {
        let data = self
            .execute(graphql::QUERY_FLASH_APPS, None, false, true)
            .await?;
        list_or_empty(&data["myself"]["flashApps"])
    }

    /// Looks up an environment with its attached endpoints and volumes.
    ///
    /// # Errors
    /// Returns request or response errors other than not-found.
    pub async fn get_environment_by_name(
        &self,
        app_name: &str,
        env_name: &str,
    ) -> Result<Option<Value>> {
        let Some(app) = self.get_app_by_name(app_name).await? else {
            return Ok(None);
        };
        match self
            .execute(
                graphql::QUERY_FLASH_ENVIRONMENT_BY_NAME,
                Some(&json!({"input":{"flashAppId":required_str(&app,"id")?,"name":env_name}})),
                false,
                true,
            )
            .await
        {
            Err(Error::Query { message, .. }) if message.to_lowercase().contains("not found") => {
                Ok(None)
            }
            result => optional_field(result?, "flashEnvironmentByName"),
        }
    }

    /// Deletes an application and returns the backend confirmation.
    ///
    /// # Errors
    /// Returns request or response errors.
    pub async fn delete_app(&self, id: &str) -> Result<bool> {
        let data = self
            .execute(
                graphql::MUTATION_DELETE_FLASH_APP,
                Some(&json!({"flashAppId":id})),
                false,
                false,
            )
            .await?;
        Ok(truthy(&data["deleteFlashApp"]))
    }

    /// Deletes an environment and returns the backend confirmation.
    ///
    /// # Errors
    /// Returns request or response errors.
    pub async fn delete_environment(&self, id: &str) -> Result<bool> {
        let data = self
            .execute(
                graphql::MUTATION_DELETE_FLASH_ENVIRONMENT,
                Some(&json!({"flashEnvironmentId":id})),
                false,
                false,
            )
            .await?;
        Ok(truthy(&data["deleteFlashEnvironment"]))
    }

    /// Opens a browser-approval login request without sending credentials.
    ///
    /// # Errors
    /// Returns request or response errors.
    pub async fn create_auth_request(&self) -> Result<Value> {
        let data = self
            .execute(
                graphql::MUTATION_CREATE_FLASH_AUTH_REQUEST,
                None,
                true,
                false,
            )
            .await?;
        Ok(data
            .get("createFlashAuthRequest")
            .filter(|v| truthy(v))
            .cloned()
            .unwrap_or_else(|| json!({})))
    }

    /// Polls an authentication request without sending an API token.
    ///
    /// # Errors
    /// Returns request or response errors after transport retries are exhausted.
    pub async fn get_auth_request_status(&self, id: &str) -> Result<Value> {
        let data = self
            .execute(
                graphql::QUERY_FLASH_AUTH_REQUEST_STATUS,
                Some(&json!({"flashAuthRequestId":id})),
                true,
                true,
            )
            .await?;
        Ok(data
            .get("flashAuthRequestStatus")
            .filter(|v| truthy(v))
            .cloned()
            .unwrap_or_else(|| json!({})))
    }

    /// Allocates a presigned artifact upload for an application and byte size.
    ///
    /// # Errors
    /// Returns request or response errors; the mutation is not retried.
    pub async fn prepare_artifact_upload(&self, app_id: &str, size: u64) -> Result<Value> {
        self.mutation(
            graphql::MUTATION_PREPARE_ARTIFACT_UPLOAD,
            &json!({"input":{"flashAppId":app_id,"tarballSize":size}}),
            "prepareFlashArtifactUpload",
        )
        .await
    }

    /// Finalizes an uploaded artifact with its resource manifest.
    ///
    /// # Errors
    /// Returns request or response errors; the mutation is not retried.
    pub async fn finalize_artifact_upload(
        &self,
        app_id: &str,
        key: &str,
        manifest: &Value,
    ) -> Result<Value> {
        self.mutation(
            graphql::MUTATION_FINALIZE_ARTIFACT_UPLOAD,
            &json!({"input":{"flashAppId":app_id,"objectKey":key,"manifest":manifest}}),
            "finalizeFlashArtifactUpload",
        )
        .await
    }

    /// Deploys a finalized build to an application environment.
    ///
    /// # Errors
    /// Returns request or response errors; deployment is not retried.
    pub async fn deploy_build(&self, environment_id: &str, build_id: &str) -> Result<Value> {
        self.mutation(
            graphql::MUTATION_DEPLOY_BUILD,
            &json!({"input":{"flashEnvironmentId":environment_id,"flashBuildId":build_id}}),
            "deployBuildToEnvironment",
        )
        .await
    }

    /// Streams a tarball to its presigned URL, reopening it on each retry.
    ///
    /// Progress receives bytes read and total bytes for each one-megabyte chunk.
    /// Each retry resets progress; pass `|_, _| {}` when no callback is needed.
    /// No account credentials are sent to the upload host. Signed URLs are redacted from errors.
    ///
    /// # Errors
    /// Returns file or HTTP errors after at most four upload attempts.
    pub async fn upload_tarball(
        &self,
        url: &str,
        path: impl AsRef<Path>,
        progress: impl Fn(u64, u64) + Send + Sync + 'static,
    ) -> Result<()> {
        let path = path.as_ref();
        let total = tokio::fs::metadata(path).await?.len();
        let progress = Arc::new(progress);
        for attempt in 0..4 {
            let result = self
                .put_tarball(url, path, total, Arc::clone(&progress))
                .await;
            match result {
                Err(Error::Http(_) | Error::Io(_)) if attempt < 3 => {
                    tokio::time::sleep(Duration::from_secs(1 << attempt)).await
                }
                result => return result,
            }
        }
        unreachable!("four upload attempts always produce a result")
    }

    async fn put_tarball(
        &self,
        url: &str,
        path: &Path,
        total: u64,
        progress: Arc<impl Fn(u64, u64) + Send + Sync + 'static>,
    ) -> Result<()> {
        let mut file = tokio::fs::File::open(path).await?;
        let reader = async_stream::try_stream! {
            let mut sent = 0;
            loop {
                let mut chunk = vec![0;1024*1024];
                let count = file.read(&mut chunk).await?;
                if count==0 {break;}
                chunk.truncate(count);
                sent+=count as u64;
                progress(sent,total);
                yield chunk;
            }
        };
        // The explicit item type keeps I/O errors attached to reqwest's body stream.
        fn body<S: futures_util::Stream<Item = std::io::Result<Vec<u8>>> + Send + 'static>(
            stream: S,
        ) -> reqwest::Body {
            reqwest::Body::wrap_stream(stream)
        }
        self.anonymous
            .http
            .put(url)
            .header("Content-Type", "application/gzip")
            .header("Content-Length", total)
            .body(body(reader))
            .timeout(Duration::from_secs(600))
            .send()
            .await
            .map_err(|e| Error::Http(e.without_url()))?
            .error_for_status()
            .map_err(|e| Error::Http(e.without_url()))?;
        Ok(())
    }
}

/// Recognizes definite capacity rejection messages used for CPU alternatives.
pub fn is_capacity_error(error: &Error) -> bool {
    let message = error.to_string().to_lowercase();
    [
        "resources to deploy your pod",
        "insufficient capacity",
        "no available machine",
        "out of stock",
    ]
    .iter()
    .any(|phrase| message.contains(phrase))
}

fn query_error(message: impl Into<String>, query: impl Into<String>) -> Error {
    Error::Query {
        message: message.into(),
        query: query.into(),
        status_code: None,
        errors: Box::new(json!([])),
    }
}
fn field(mut value: Value, key: &str) -> Result<Value> {
    value
        .as_object_mut()
        .and_then(|o| o.remove(key))
        .ok_or_else(|| Error::InvalidResponse(format!("missing field {key}")))
}
fn optional_field(value: Value, key: &str) -> Result<Option<Value>> {
    let value = field(value, key)?;
    Ok((!value.is_null()).then_some(value))
}
fn list(value: Value) -> Result<Vec<Value>> {
    match value {
        Value::Array(values) => Ok(values),
        _ => Err(Error::InvalidResponse("expected a list".into())),
    }
}
fn list_or_empty(value: &Value) -> Result<Vec<Value>> {
    if !truthy(value) {
        Ok(Vec::new())
    } else {
        list(value.clone())
    }
}
fn optional_string(value: &Value) -> Result<Option<String>> {
    if value.is_null() {
        Ok(None)
    } else {
        value
            .as_str()
            .map(|s| Some(s.to_owned()))
            .ok_or_else(|| Error::InvalidResponse("expected string or null".into()))
    }
}
fn stock_in_datacenter(data: &Value, id: &str) -> Result<Option<String>> {
    if let Some(entry) = data
        .get("dataCenters")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|dc| dc["id"] == id))
    {
        optional_string(&entry["availability"])
    } else {
        Ok(Some("NONE".into()))
    }
}
