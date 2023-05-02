// src/api_wrapper/mutations/pods.rs

use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug)]
pub struct PodCreateInput {
    pub name: String,
    pub image_name: String,
    pub gpu_type_id: String,
    pub cloud_type: Option<String>,
    pub gpu_count: Option<i32>,
    pub volume_in_gb: Option<i32>,
    pub container_disk_in_gb: Option<i32>,
    pub min_vcpu_count: Option<i32>,
    pub min_memory_in_gb: Option<i32>,
    pub docker_args: Option<String>,
    pub ports: Option<String>,
    pub volume_mount_path: Option<String>,
    pub env: Option<HashMap<String, String>>,
}

pub fn generate_pod_deployment_mutation(input: PodCreateInput) -> String {
    let mut input_fields = Vec::new();

    if let Some(cloud_type) = input.cloud_type {
        input_fields.push(format!("cloudType: {}", cloud_type));
    }
    if let Some(gpu_count) = input.gpu_count {
        input_fields.push(format!("gpuCount: {}", gpu_count));
    }
    if let Some(volume_in_gb) = input.volume_in_gb {
        input_fields.push(format!("volumeInGb: {}", volume_in_gb));
    }
    if let Some(container_disk_in_gb) = input.container_disk_in_gb {
        input_fields.push(format!("containerDiskInGb: {}", container_disk_in_gb));
    }
    if let Some(min_vcpu_count) = input.min_vcpu_count {
        input_fields.push(format!("minVcpuCount: {}", min_vcpu_count));
    }
    if let Some(min_memory_in_gb) = input.min_memory_in_gb {
        input_fields.push(format!("minMemoryInGb: {}", min_memory_in_gb));
    }
    input_fields.push(format!(r#"gpuTypeId: "{}""#, input.gpu_type_id));
    input_fields.push(format!(r#"name: "{}""#, input.name));
    input_fields.push(format!(r#"imageName: "{}""#, input.image_name));
    if let Some(docker_args) = input.docker_args {
        input_fields.push(format!(r#"dockerArgs: "{}""#, docker_args));
    }
    if let Some(ports) = input.ports {
        input_fields.push(format!(r#"ports: "{}""#, ports));
    }
    if let Some(volume_mount_path) = input.volume_mount_path {
        input_fields.push(format!(r#"volumeMountPath: "{}""#, volume_mount_path));
    }
    if let Some(env) = input.env {
        let env_string = env
            .iter()
            .map(|(key, value)| format!(r#"{{ key: "{}", value: "{}" }}"#, key, value))
            .collect::<Vec<String>>()
            .join(", ");
        input_fields.push(format!("env: [{}]", env_string));
    }

    let input_string = input_fields.join(", ");

    format!(
        r#"
    mutation {{
      podFindAndDeployOnDemand(
        input: {{
          {0}
        }}
      ) {{
        id
        imageName
        env
        machineId
        machine {{
          podHostId
        }}
      }}
    }}
    "#,
        input_string
    )
}

pub fn generate_pod_stop_mutation(pod_id: &str) -> String {
    format!(
        r#"
    mutation {{
        podStop(input: {{ podId: "{0}" }}) {{
            id
            desiredStatus
        }}
    }}
    "#,
        pod_id
    )
}

pub fn generate_pod_resume_mutation(pod_id: &str, gpu_count: i32) -> String {
    format!(
        r#"
    mutation {{
        podResume(input: {{ podId: "{0}", gpuCount: {1} }}) {{
            id
            desiredStatus
            imageName
            env
            machineId
            machine {{
                podHostId
            }}
        }}
    }}
    "#,
        pod_id,
        gpu_count
    )
}

pub fn generate_pod_terminate_mutation(pod_id: &str) -> String {
    format!(
        r#"
    mutation {{
        podTerminate(input: {{ podId: "{0}" }})
    }}
    "#,
        pod_id
    )
}

