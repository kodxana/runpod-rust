use runpod::{
    apps::{
        DataCenter, EnvValue, GpuGroup, GpuType, Model, Secret,
        gpu::{gpu_ids_value, needs_gpu_catalog, resolve_gpu_string},
        render_env, secret_names,
    },
    json,
};

#[test]
fn model_reference_and_paths() {
    let model = Model::new("owner/model:revision").unwrap();
    assert_eq!(model.reference(), "owner/model:revision");
    assert!(model.hf_cache_path().ends_with("models--owner--model"));
    for invalid in ["", "model", "a/b/c", "owner/name space"] {
        assert!(Model::new(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn secret_references_and_env_order() {
    let secret = Secret::new("hf-token").unwrap();
    assert_eq!(secret.reference(), "{{ RUNPOD_SECRET_hf-token }}");
    let env = vec![
        ("HF_TOKEN".into(), EnvValue::from(secret)),
        ("A".into(), EnvValue::from("value")),
    ];
    assert_eq!(secret_names(&env), vec!["hf-token"]);
    assert_eq!(
        render_env(&env),
        json!({"HF_TOKEN":"{{ RUNPOD_SECRET_hf-token }}","A":"value"})
            .as_object()
            .unwrap()
            .clone()
    );
    for invalid in ["", "space name", "name/other", "ł"] {
        assert!(Secret::new(invalid).is_err());
    }
}

#[test]
fn gpu_resolution_and_device_exclusions() {
    assert_eq!(
        resolve_gpu_string("4090").unwrap(),
        vec!["NVIDIA GeForce RTX 4090"]
    );
    assert_eq!(resolve_gpu_string("ampere_80").unwrap(), vec!["AMPERE_80"]);
    assert_eq!(resolve_gpu_string("h100").unwrap().len(), 3);
    assert_eq!(resolve_gpu_string("").unwrap().len(), GpuType::all().len());
    assert!(resolve_gpu_string("invalid gpu").is_err());
    let selected = vec!["NVIDIA H100 PCIe".into()];
    assert!(needs_gpu_catalog(&selected));
    assert_eq!(
        gpu_ids_value(&selected, None),
        "ADA_80_PRO,-NVIDIA H100 80GB HBM3,-NVIDIA H100 NVL"
    );
    let selected = vec!["NVIDIA H100 PCIe".into(), "ADA_80_PRO".into()];
    assert!(!needs_gpu_catalog(&selected));
    assert_eq!(gpu_ids_value(&selected, None), "ADA_80_PRO");
    assert_eq!(
        gpu_ids_value(&[], None).split(',').count(),
        GpuGroup::all().len()
    );
}

#[test]
fn datacenter_normalization_and_catalog_roundtrips() {
    assert_eq!(
        " us_ca_2 ".parse::<DataCenter>().unwrap().to_string(),
        "US-CA-2"
    );
    assert!("invalid".parse::<DataCenter>().is_err());
    for gpu in GpuType::all() {
        assert_eq!(gpu.as_str().parse::<GpuType>().unwrap(), *gpu);
    }
}
