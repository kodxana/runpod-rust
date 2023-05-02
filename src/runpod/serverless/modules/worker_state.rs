use std::env;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

lazy_static::lazy_static! {
    static ref CURRENT_JOB_ID: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    static ref WORKER_ID: String = env::var("RUNPOD_POD_ID").unwrap_or_else(|_| Uuid::new_v4().to_string());
}

pub fn get_auth_header() -> String {
    env::var("RUNPOD_AI_API_KEY").unwrap_or_default()
}

pub fn job_get_url() -> String {
    env::var("RUNPOD_WEBHOOK_GET_JOB")
        .unwrap_or_default()
        .replace("$ID", &*WORKER_ID)
}

pub fn job_done_url_template() -> String {
    env::var("RUNPOD_WEBHOOK_POST_OUTPUT")
        .unwrap_or_default()
        .replace("$RUNPOD_POD_ID", &*WORKER_ID)
}

pub fn webhook_ping() -> Option<String> {
    env::var("RUNPOD_WEBHOOK_PING")
        .map(|url| url.replace("$RUNPOD_POD_ID", &*WORKER_ID))
        .ok()
}

pub fn ping_interval() -> u64 {
    env::var("RUNPOD_PING_INTERVAL")
        .map(|interval| interval.parse::<u64>().unwrap_or(10000))
        .unwrap_or(10000)
}

pub fn get_current_job_id() -> Option<String> {
    CURRENT_JOB_ID.lock().unwrap().clone()
}

pub fn get_done_url() -> String {
    job_done_url_template().replace("$ID", &*get_current_job_id().unwrap())
}

pub fn set_job_id(new_job_id: Option<String>) {
    *CURRENT_JOB_ID.lock().unwrap() = new_job_id;
}
