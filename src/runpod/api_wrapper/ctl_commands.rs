// src/api_wrapper/ctl_commands.rs

use super::graphql::run_graphql_query;
use super::mutations::pods::{self, PodCreateInput};
use super::queries::gpus::{self, GpuQuery};
use serde_json::Value;
use std::collections::HashMap;

pub async fn get_gpus() -> Result<Vec<Value>, reqwest::Error> {
    let raw_return = run_graphql_query(&gpus::QUERY_GPU_TYPES).await?;
    let cleaned_return = raw_return["data"]["gpuTypes"].as_array().unwrap().clone();
    Ok(cleaned_return)
}

pub async fn get_gpu(gpu_id: &str) -> Result<Value, reqwest::Error> {
    let query = gpus::generate_gpu_query(gpu_id);
    let raw_return = run_graphql_query(&query).await?;
    let cleaned_return = raw_return["data"]["gpuTypes"][0].clone();
    Ok(cleaned_return)
}

pub async fn create_pod(input: PodCreateInput) -> Result<Value, reqwest::Error> {
    let query = pods::generate_pod_deployment_mutation(input);
    let raw_response = run_graphql_query(&query).await?;
    let cleaned_response = raw_response["data"]["podFindAndDeployOnDemand"].clone();
    Ok(cleaned_response)
}

pub async fn stop_pod(pod_id: &str) -> Result<Value, reqwest::Error> {
    let query = pods::generate_pod_stop_mutation(pod_id);
    let raw_response = run_graphql_query(&query).await?;
    let cleaned_response = raw_response["data"]["podStop"].clone();
    Ok(cleaned_response)
}

pub async fn resume_pod(pod_id: &str, gpu_count: i32) -> Result<Value, reqwest::Error> {
    let query = pods::generate_pod_resume_mutation(pod_id, gpu_count);
    let raw_response = run_graphql_query(&query).await?;
    let cleaned_response = raw_response["data"]["podResume"].clone();
    Ok(cleaned_response)
}

pub async fn terminate_pod(pod_id: &str) -> Result<(), reqwest::Error> {
    let query = pods::generate_pod_terminate_mutation(pod_id);
    run_graphql_query(&query).await?;
    Ok(())
}
