use runpod::{
    apps::{
        GlobalVolume, Mount, NetworkVolume, ResourceKind, ResourceSpec, Volume,
        volume::{ComputeType, MountBindings, normalize_mounts, validate_mounts},
    },
    json,
};

#[test]
fn mount_validation_matches_runtime_capabilities() {
    let network = Volume::from(NetworkVolume::new("data").unwrap());
    let global = Volume::from(GlobalVolume::new("objects").unwrap());
    let mounts = normalize_mounts(&[Mount {
        path: "/runpod-volume/./".into(),
        volume: network.clone(),
    }])
    .unwrap();
    assert_eq!(mounts[0].path, "/runpod-volume");
    validate_mounts(&mounts, "queue", ComputeType::Gpu).unwrap();
    assert!(
        normalize_mounts(&[Mount {
            path: "/bad/../path".into(),
            volume: network.clone()
        }])
        .is_err()
    );
    assert!(
        validate_mounts(
            &[
                Mount {
                    path: "/a".into(),
                    volume: network.clone()
                },
                Mount {
                    path: "/a/b".into(),
                    volume: global.clone()
                }
            ],
            "task",
            ComputeType::Gpu
        )
        .is_err()
    );
    assert!(
        validate_mounts(
            &[Mount {
                path: "/runpod-volume".into(),
                volume: global
            }],
            "queue",
            ComputeType::Cpu
        )
        .is_err()
    );
    let mut spec = ResourceSpec::new(ResourceKind::Queue, "test");
    spec.mounts = mounts;
    assert_eq!(
        spec.to_manifest().unwrap()["mounts"],
        json!([{"kind":"network","reference":"data","path":"/runpod-volume"}])
    );
}

#[test]
fn mount_bindings_resolve_name_and_id_and_reject_ambiguity() {
    let bindings = MountBindings::from_value(
        &json!([{"kind":"network","reference":"data","id":"vol-id","path":"/data"}]),
    )
    .unwrap();
    assert_eq!(bindings.path("network", "data").unwrap(), "/data");
    assert_eq!(bindings.path("network", "vol-id").unwrap(), "/data");
    assert!(bindings.path("global", "data").is_err());
    let bindings=MountBindings::from_value(&json!([{"kind":"network","reference":"data","id":"id","path":"/a"},{"kind":"network","reference":"data","id":"id","path":"/b"}])).unwrap();
    assert!(
        bindings
            .path("network", "data")
            .unwrap_err()
            .to_string()
            .contains("multiple mounts")
    );
}
