use std::collections::HashMap;
use reqwest::{Client, Response};
use serde_json::Value;
use tokio::time::{sleep, Duration};

pub struct Job {
    pub endpoint_id: String,
    pub job_id: String,
    pub status_url: String,
    pub client: Client,
}

impl Job {
    pub async fn status(&self) -> String {
        let response = self.client.get(&self.status_url).send().await.unwrap();
        let json: Value = response.json().await.unwrap();
        json["status"].as_str().unwrap().to_string()
    }

    pub async fn output(self) -> Value {
        loop {
            let status = self.status().await;
            if status == "COMPLETED" || status == "FAILED" {
                break;
            }
            sleep(Duration::from_secs(1)).await;
        }
        let response = self.client.get(&self.status_url).send().await.unwrap();
        let json: Value = response.json().await.unwrap();
        json["output"].clone()
    }
}

pub struct Endpoint {
    pub endpoint_id: String,
    pub endpoint_url: String,
    pub client: Client,
}

impl Endpoint {
    pub async fn run(&self, endpoint_input: HashMap<String, Value>) -> Job {
        let response = self
            .client
            .post(&self.endpoint_url)
            .json(&json!({ "input": endpoint_input }))
            .send()
            .await
            .unwrap();
        let json_resp: Value = response.json().await.unwrap();
        let job_id = json_resp["id"].as_str().unwrap().to_string();
        let status_url = format!("{}/{}/status/{}", crate::ENDPOINT_URL_BASE, self.endpoint_id, job_id);
        Job {
            endpoint_id: self.endpoint_id.clone(),
            job_id,
            status_url,
            client: self.client.clone(),
        }
    }
}
