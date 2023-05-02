// src/api_wrapper/graphql.rs

use reqwest::header;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub async fn run_graphql_query(query: &str) -> Result<HashMap<String, serde_json::Value>, reqwest::Error> {
    let api_key = match std::env::var("API_KEY") {
        Ok(key) => key,
        Err(_) => panic!("API_KEY environment variable not set."),
    };

    let url = format!("https://api.runpod.io/graphql?api_key={}", api_key);

    let client = reqwest::Client::new();
    let query_data = QueryData { query: query.to_string() };
    let response = client
        .post(&url)
        .header(header::CONTENT_TYPE, "application/json")
        .json(&query_data)
        .send()
        .await?;

    let json_result: HashMap<String, serde_json::Value> = response.json().await?;
    Ok(json_result)
}

#[derive(Serialize, Deserialize)]
struct QueryData {
    query: String,
}
