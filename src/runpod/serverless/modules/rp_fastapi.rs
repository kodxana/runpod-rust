use std::env;
use std::sync::{Arc, Mutex};
use tokio::task;
use uuid::Uuid;
use warp::Filter;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::heartbeat::start_ping;

#[derive(Deserialize, Serialize, Debug)]
pub struct Job {
    id: String,
    input: serde_json::Value,
}

pub struct WorkerAPI<F> {
    handler: Arc<Mutex<F>>,
}

impl<F, Fut> WorkerAPI<F>
where
    F: Fn(Job) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = serde_json::Value> + Send + 'static,
{
    pub fn new(handler: F) -> Self {
        // Start the heartbeat thread.
        task::spawn(start_ping());

        // Set the handler for processing jobs.
        WorkerAPI {
            handler: Arc::new(Mutex::new(handler)),
        }
    }

    pub async fn run(&self, job: Job) -> Result<impl warp::Reply, warp::Rejection> {
        let handler = self.handler.lock().unwrap();

        // Process the job using the provided handler.
        let job_results = handler(job).await;

        // Return the results of the job processing.
        Ok(warp::reply::json(&job_results))
    }

    pub async fn start_server(self, api_port: u16, api_concurrency: usize) {
        let worker_api = Arc::new(self);

        let job_route = warp::path!(String / "realtime")
            .and(warp::post())
            .and(warp::body::json())
            .and_then(move |endpoint_id: String, job: Job| {
                let worker_api = Arc::clone(&worker_api);
                async move {
                    let rp_endpoint_id = env::var("RUNPOD_ENDPOINT_ID").unwrap_or_default();

                    if endpoint_id == rp_endpoint_id {
                        worker_api.run(job).await
                    } else {
                        Err(warp::reject::not_found())
                    }
                }
            });

        let routes = job_route;

        warp::serve(routes).run(([0, 0, 0, 0], api_port)).await;
    }
}

// Example usage:

async fn handler(job: Job) -> serde_json::Value {
    json!({"job_id": job.id, "job_input": job.input})
}

#[tokio::main]
async fn main() {
    let api_port = env::var("API_PORT").unwrap_or_else(|_| "8000".to_string()).parse().unwrap();
    let api_concurrency = env::var("API_CONCURRENCY")
        .unwrap_or_else(|_| "1".to_string())
        .parse()
        .unwrap();

    let worker_api = WorkerAPI::new(handler);
    worker_api.start_server(api_port, api_concurrency).await;
}
