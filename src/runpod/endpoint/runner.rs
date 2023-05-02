use std::collections::HashMap;
use reqwest::{Client, Response};
use serde_json::Value;
use std::time::Duration;
use std::thread;

pub struct Job {
    pub endpoint_id: String,
    pub job_id: String,
    pub status_url: String,
    pub client: Client,
}

impl Job {
    pub fn status(&self) -> String {
        let response = self.client.get(&self.status_url).send().unwrap();
        let json: Value = response.json().unwrap();
        json["status"].as_str().unwrap().to_string()
    }

    pub fn output(mut self) -> Value {
        while self.status() != "COMPLETED" && self.status() != "FAILED" {
            thread::sleep(Duration::from_millis(100));
        }
        let response = self.client.get(&self.status_url).send().unwrap();
        let json: Value = response.json().unwrap();
        json["output"].clone()
    }
}

pub struct Endpoint {
    pub endpoint_id: String,
    pub endpoint_url: String,
    pub client: Client,
}

impl Endpoint {
    pub fn run(&self, endpoint_input: HashMap<String, Value>) -> Job {
        let response = self
            .client
            .post(&self.endpoint_url)
            .json(&json!({ "input": endpoint_input }))
            .send()
            .unwrap();
        let json_resp: Value = response.json().unwrap();
        let job_id = json_resp["id"].as_str().unwrap().to_string();
        let status_url = format!("{}/{}/status/{}", crate::ENDPOINT_URL_BASE, self.endpoint_id, job_id);
        Job {
            endpoint_id: self.endpoint_id.clone(),
            job_id,
            status_url,
            client: self.client.clone(),
        }
    }

    pub fn run_sync(&self, endpoint_input: HashMap<String, Value>) -> Value {
        let response = self
            .client
            .post(&self.endpoint_url)
            .json(&json!({ "input": endpoint_input }))
            .send()
            .unwrap();
        let json_resp: Value = response.json().unwrap();
        json_resp.clone()
    }
}
