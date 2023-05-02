use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;
use reqwest::{Client, StatusCode};
use log::{info, warn, error, debug};

use crate::worker_state::{JOB_GET_URL, get_done_url};
use crate::retry::{retry, RetryConfig};
use crate::rp_tips::check_return_size;

pub async fn get_local() -> Option<Value> {
    if let Ok(content) = fs::read_to_string("test_input.json") {
        if let Ok(test_inputs) = serde_json::from_str(&content) {
            let mut test_inputs: Value = test_inputs;
            if !test_inputs.as_object().unwrap().contains_key("id") {
                test_inputs.as_object_mut().unwrap().insert("id".into(), "local_test".into());
            }
            debug!("Retrieved local job: {:?}", test_inputs);
            return Some(test_inputs);
        }
    }
    warn!("test_input.json not found, skipping local testing");
    None
}

pub async fn get_job(client: Arc<Client>) -> Option<Value> {
    let next_job: Value;

    if let Some(job_get_url) = JOB_GET_URL.as_ref() {
        let response = match client.get(job_get_url).send().await {
            Ok(resp) => resp,
            Err(err) => {
                error!("Error while getting job: {:?}", err);
                return None;
            }
        };
        next_job = match response.json().await {
            Ok(json) => json,
            Err(err) => {
                error!("Error while parsing job JSON: {:?}", err);
                return None;
            }
        };
    } else {
        warn!("RUNPOD_WEBHOOK_GET_JOB not set, switching to get_local");
        return get_local().await;
    }

    info!("Received job: {:?}", next_job["id"]);
    Some(next_job)
}

pub fn run_job<F: Fn(Value) -> Value>(handler: F, job: Value) -> Value {
    let start_time = Instant::now();
    info!("Started working on job {:?} at {:?} UTC", job["id"], start_time);

    let run_result = handler(job);
    debug!("Job handler output: {:?}", run_result);

    let mut run_result = if run_result.is_bool() {
        json!({ "output": run_result })
    } else if run_result.as_object().unwrap().contains_key("error") {
        json!({ "error": run_result["error"].to_string() })
    } else if run_result.as_object().unwrap().contains_key("refresh_worker") {
        run_result.as_object_mut().unwrap().remove("refresh_worker");
        json!({ "stopPod": true, "output": run_result })
    } else {
        json!({ "output": run_result })
    };

    check_return_size(&run_result);

    let end_time = Instant::now();
    info!("Finished working on job {:?} at {:?} UTC", job["id"], end_time);
    info!("Job took {:?} seconds to complete", end_time.duration_since(start_time));
    debug!("Run result: {:?}", run_result);

    run_result
}

pub async fn retry_send_result(client: Arc<Client>, job_data: &str) -> Result<(), reqwest::Error> {
    let config = RetryConfig {
        max_attempts: 3,
        base_delay: 1,
        max_delay: 3,
    };
    retry(config, || async {
        let resp = client
            .post(get_done_url().as_str())
            .header("charset", "utf-8")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(job_data.to_string())
            .send()
            .await?;

        if resp.status() != StatusCode::OK {
            return Err(reqwest::Error::Status(resp.status()));
        }

        debug!("Result API response: {:?}", resp.text().await);
        Ok(())
    })
    .await
}

pub async fn send_result(client: Arc<Client>, mut job_data: Value, job: &Value) {
    if let Err(err) = serde_json::to_string(&job_data) {
        error!("Error while serializing job result {:?}: {:?}", job["id"], err);
        return;
    }

    if JOB_GET_URL.is_some() {
        info!("Sending job results for {:?}: {:?}", job["id"], job_data);
        if let Err(err) = retry_send_result(client.clone(), &serde_json::to_string(&job_data).unwrap()).await {
            error!("Error while returning job result {:?}: {:?}", job["id"], err);
        } else {
            info!("Successfully returned job result {:?}", job["id"]);
        }
    } else {
        warn!("Local test job results for {:?}: {:?}", job["id"], job_data);
    }
}
