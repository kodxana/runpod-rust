pub const QUERY_GPU_TYPES: &str = r#"
query GpuTypes {
  gpuTypes {
    id
    displayName
    memoryInGb
  }
}
"#;

pub fn generate_gpu_query(gpu_id: &str) -> String {
    format!(
        r#"
    query GpuTypes {{
      gpuTypes(input: {{id: "{0}"}}) {{
        id
        displayName
        memoryInGb
        secureCloud
        communityCloud
        lowestPrice(input: {{gpuCount: 1}}) {{
          minimumBidPrice
          uninterruptablePrice
        }}
      }}
    }}
    "#,
        gpu_id
    )
}
