use crate::{Result, Value, client::truthy, json};
use std::{collections::BTreeMap, fmt, time::Duration};

/// Pod-scoped deletion acknowledgement with the runtime's bounded retry policy.
#[derive(Clone)]
pub struct PodTermination {
    client: reqwest::Client,
    pod_id: String,
    api_key: String,
    api_base: String,
    task: bool,
}
impl fmt::Debug for PodTermination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PodTermination")
            .field("pod_id", &self.pod_id)
            .field("task", &self.task)
            .finish_non_exhaustive()
    }
}
enum Outcome {
    Acknowledged,
    Retry,
    Rejected,
}
impl PodTermination {
    /// Captures pod identity, credentials, runtime kind and API origin without making requests.
    ///
    /// # Errors
    /// Returns HTTP client configuration failures.
    pub fn from_env() -> Result<Self> {
        Self::from_values(
            &[
                "RUNPOD_POD_ID",
                "RUNPOD_API_KEY",
                "RUNPOD_API_BASE_URL",
                "RUNPOD_RUNTIME_KIND",
            ]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
            .collect(),
        )
    }
    /// Uses explicit environment values; missing pod ID or key disables deletion requests.
    ///
    /// # Errors
    /// Returns HTTP client configuration failures.
    pub fn from_values(values: &BTreeMap<String, String>) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()?,
            pod_id: values.get("RUNPOD_POD_ID").cloned().unwrap_or_default(),
            api_key: values.get("RUNPOD_API_KEY").cloned().unwrap_or_default(),
            api_base: values
                .get("RUNPOD_API_BASE_URL")
                .cloned()
                .unwrap_or_else(|| "https://api.runpod.io".into()),
            task: values
                .get("RUNPOD_RUNTIME_KIND")
                .is_some_and(|value| value == "task"),
        })
    }

    /// Requests deletion up to three times, returning true only for the exact acknowledgement.
    ///
    /// Task deletion requires `flashTaskTerminate: true` and sends the pod identity header.
    /// Legacy pod deletion requires an explicitly present `podTerminate: null`. Only transport
    /// failures, HTTP 429/5xx and recognized transient GraphQL errors retry, after 0.5 and 1 seconds.
    /// This method never exits the process.
    ///
    /// # Errors
    /// Returns a final transport/build failure with its native cause. Rejected or malformed
    /// control-plane responses return false so the watchdog can retry later.
    pub async fn terminate(&self) -> Result<bool> {
        if self.pod_id.is_empty() || self.api_key.is_empty() {
            return Ok(false);
        }
        for attempt in 0..3 {
            match self.attempt().await {
                Ok(Outcome::Acknowledged) => return Ok(true),
                Ok(Outcome::Rejected) => return Ok(false),
                Ok(Outcome::Retry) if attempt == 2 => return Ok(false),
                Err(error) if attempt == 2 || error.is_builder() => return Err(error.into()),
                _ => {}
            }
            tokio::time::sleep(Duration::from_millis(500 * (1 << attempt))).await;
        }
        Ok(false)
    }
    async fn attempt(&self) -> std::result::Result<Outcome, reqwest::Error> {
        let (mutation, input) = if self.task {
            ("flashTaskTerminate", "FlashTaskTerminateInput")
        } else {
            ("podTerminate", "PodTerminateInput")
        };
        let payload = json!({"query":format!("mutation {mutation}($input: {input}!) {{ {mutation}(input: $input) }}"),"variables":{"input":{"podId":self.pod_id}}});
        let mut request = self
            .client
            .post(format!("{}/graphql", self.api_base.trim_end_matches('/')))
            .bearer_auth(&self.api_key)
            .json(&payload);
        if self.task {
            request = request.header("X-RunPod-Pod-Id", &self.pod_id);
        }
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            return Ok(if status.as_u16() == 429 || status.is_server_error() {
                Outcome::Retry
            } else {
                Outcome::Rejected
            });
        }
        let bytes = response.bytes().await?;
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            return Ok(Outcome::Rejected);
        };
        let errors = value.get("errors").filter(|errors| truthy(errors));
        let result = value.get("data").and_then(|data| data.get(mutation));
        if errors.is_none()
            && result.is_some_and(|value| {
                if self.task {
                    value == true
                } else {
                    value.is_null()
                }
            })
        {
            return Ok(Outcome::Acknowledged);
        }
        let retry = errors.and_then(Value::as_array).is_some_and(|errors| {
            !errors.is_empty()
                && errors.iter().all(|error| {
                    error
                        .get("extensions")
                        .and_then(|ext| ext.get("code"))
                        .and_then(Value::as_str)
                        .is_some_and(|code| {
                            [
                                "INTERNAL_SERVER_ERROR",
                                "SERVICE_UNAVAILABLE",
                                "TOO_MANY_REQUESTS",
                                "TIMEOUT",
                            ]
                            .contains(&code)
                        })
                })
        });
        Ok(if retry {
            Outcome::Retry
        } else {
            Outcome::Rejected
        })
    }
}
