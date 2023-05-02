use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::Duration;
use std::thread;

use reqwest::{Client, StatusCode};
use log::{info, error, debug};

use crate::worker_state::{get_current_job_id, PING_URL, PING_INTERVAL};

pub fn send_ping(client: Arc<Client>, ping_params: Option<HashMap<String, String>>) {
    if let Some(ping_url) = PING_URL.as_ref() {
        let result = match client.get(ping_url).query(&ping_params).send() {
            Ok(res) => res,
            Err(err) => {
                error!("Heartbeat Failed  URL: {}  Params: {:?}", ping_url, ping_params);
                error!("Heartbeat Fail  Error: {:?}", err);
                return;
            }
        };

        info!("Heartbeat Sent  URL: {}  Status: {:?}", ping_url, result.status());
        info!("Heartbeat Sent  Interval: {}ms  Params: {:?}", PING_INTERVAL.as_millis(), ping_params);
    }
}

pub fn start_ping(client: Arc<Client>) {
    let job_id = get_current_job_id();

    let mut ping_params = HashMap::new();
    if let Some(job_id) = job_id {
        ping_params.insert("job_id".to_string(), job_id);
    }

    send_ping(client.clone(), Some(ping_params));

    debug!("Scheduling next heartbeat in {}ms", PING_INTERVAL.as_millis());
    thread::spawn(move || {
        loop {
            thread::sleep(PING_INTERVAL);
            start_ping(client.clone());
        }
    });
}
