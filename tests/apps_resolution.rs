//! Ports of storage resolution and attachment behavior from test_apps/test_volume.py.
use runpod::{
    Client, ClientConfig, Value,
    apps::{
        AppsApiClient, GlobalVolume, Mount, NetworkVolume, ResourceKind, ResourceSpec, Volume,
        volume::{MountBindings, VolumeResolver, attach_endpoint_volumes, attach_pod_mounts},
    },
    json,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path},
};

fn resolver(server: &MockServer) -> VolumeResolver {
    VolumeResolver::new(
        AppsApiClient::new(
            Client::with_config(ClientConfig {
                api_base_url: server.uri(),
                ..ClientConfig::new("key")
            })
            .unwrap(),
        )
        .unwrap(),
    )
}

async fn volume_list(server: &MockServer, backend: &str, volumes: Value) {
    Mock::given(method("GET"))
        .and(path(format!("/v2/{backend}-volumes")))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({format!("{backend}Volumes"):volumes,"pagination":{"hasNextPage":false}}),
        ))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn test_existing_by_name_and_id() {
    for reference in ["models", "nv-1"] {
        let server = MockServer::start().await;
        volume_list(
            &server,
            "network",
            json!([{"id":"nv-1","name":"models","dataCenter":"EU-RO-1"}]),
        )
        .await;
        let mut volume = NetworkVolume::new(reference).unwrap();
        volume.datacenter = Some("US-IL-1".into());
        let volume: Volume = volume.into();
        let mut resolver = resolver(&server);
        let specs = [ResourceSpec::new(ResourceKind::Task, "task")];
        let resolved = resolver.resolve(&volume, &specs).await.unwrap();
        assert_eq!(
            json!(resolved),
            json!({"id":"nv-1","dataCenterId":"EU-RO-1"})
        );
        assert_eq!(resolver.resolve(&volume, &specs).await.unwrap(), resolved);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn test_duplicate_names_raise_and_ids_take_precedence() {
    for backend in ["network", "global"] {
        let server = MockServer::start().await;
        volume_list(&server,backend,json!([{"id":"a","name":"models","dataCenter":"EU-RO-1"},{"id":"b","name":"models","dataCenter":"US-KS-2"}])).await;
        let volume = if backend == "network" {
            NetworkVolume::new("models").unwrap().into()
        } else {
            GlobalVolume::new("models").unwrap().into()
        };
        assert!(
            resolver(&server)
                .resolve(&volume, &[])
                .await
                .unwrap_err()
                .to_string()
                .contains("reference by id")
        );
    }
    let items = vec![
        json!({"id":"id","name":"target"}),
        json!({"id":"target","name":"other"}),
    ];
    assert_eq!(
        runpod::apps::lookup::find_by_id_or_name(&items, "target", "volumes").unwrap(),
        Some(&items[1])
    );
}

#[tokio::test]
async fn test_missing_no_create_raises() {
    for backend in ["network", "global"] {
        let server = MockServer::start().await;
        volume_list(&server, backend, json!([])).await;
        let volume = if backend == "network" {
            let mut v = NetworkVolume::new("models").unwrap();
            v.create = false;
            v.into()
        } else {
            let mut v = GlobalVolume::new("models").unwrap();
            v.create = false;
            v.into()
        };
        assert!(
            resolver(&server)
                .resolve(&volume, &[])
                .await
                .unwrap_err()
                .to_string()
                .contains("create=False")
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn test_global_creation_is_cached_and_mounts_resolve_by_name_and_id() {
    let server = MockServer::start().await;
    volume_list(&server, "global", json!([])).await;
    Mock::given(method("POST"))
        .and(path("/v2/global-volumes"))
        .and(body_json(json!({"name":"models"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"gv-new"})))
        .expect(1)
        .mount(&server)
        .await;
    let volume: Volume = GlobalVolume::new("models").unwrap().into();
    let mut resolver = resolver(&server);
    let resolved = resolver
        .resolve_mounts(
            &[Mount {
                path: "/models".into(),
                volume: volume.clone(),
            }],
            &[],
        )
        .await
        .unwrap();
    assert_eq!(resolver.resolve(&volume, &[]).await.unwrap().id, "gv-new");
    let bindings = MountBindings::from_value(&json!(resolved)).unwrap();
    assert_eq!(bindings.path("global", "models").unwrap(), "/models");
    assert_eq!(bindings.path("global", "gv-new").unwrap(), "/models");
}

#[tokio::test]
async fn test_creation_without_app_requires_explicit_placement() {
    let server = MockServer::start().await;
    volume_list(&server, "network", json!([])).await;
    let volume: Volume = NetworkVolume::new("models").unwrap().into();
    assert!(
        resolver(&server)
            .resolve(&volume, &[])
            .await
            .unwrap_err()
            .to_string()
            .contains("requires a datacenter")
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn test_unsupported_explicit_pin_never_relocates() {
    let server = MockServer::start().await;
    volume_list(&server, "network", json!([])).await;
    Mock::given(method("GET"))
        .and(path("/v2/catalog/datacenters"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"dataCenters":[{"id":"EU-RO-1","networkVolumeTypes":["STANDARD"]}]}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let mut volume = NetworkVolume::new("models").unwrap();
    volume.datacenter = Some("US-IL-1".into());
    assert!(
        resolver(&server)
            .resolve(&volume.into(), &[])
            .await
            .unwrap_err()
            .to_string()
            .contains("does not support network volumes")
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn test_storage_capability_beats_higher_hardware_stock() {
    let server = MockServer::start().await;
    volume_list(&server, "network", json!([])).await;
    Mock::given(path("/v2/catalog/datacenters")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"dataCenters":[{"id":"EU-RO-1","networkVolumeTypes":["STANDARD"]},{"id":"US-IL-1","networkVolumeTypes":[]}]}))).expect(1).mount(&server).await;
    Mock::given(path("/v2/catalog/cpus/cpu3c")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"dataCenters":[{"id":"US-IL-1","availability":"HIGH"},{"id":"EU-RO-1","availability":"LOW"}]}))).expect(runpod::apps::DataCenter::all().len() as u64).mount(&server).await;
    Mock::given(method("POST"))
        .and(path("/v2/network-volumes"))
        .and(body_json(
            json!({"name":"models","size":50,"dataCenter":"EU-RO-1"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"nv-new"})))
        .expect(1)
        .mount(&server)
        .await;
    let mut spec = ResourceSpec::new(ResourceKind::Task, "task");
    spec.cpu = Some("cpu3c-2-4".into());
    let volume: Volume = NetworkVolume::new("models").unwrap().into();
    let mut resolver = resolver(&server);
    use std::sync::{Arc, Mutex};
    let events = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&events);
    resolver.set_events(runpod::apps::events::EventSink::new(move |name, args| {
        record
            .lock()
            .unwrap()
            .push((name.to_owned(), args.to_vec()));
        Err(runpod::Error::InvalidInput("renderer failed".into()))
    }));
    assert_eq!(
        json!(resolver.resolve(&volume, &[spec]).await.unwrap()),
        json!({"id":"nv-new","dataCenterId":"EU-RO-1"})
    );
    assert_eq!(
        events.lock().unwrap()[0],
        (
            "volume_created".into(),
            vec![json!("models"), json!(50), json!("EU-RO-1")]
        )
    );
}

#[tokio::test]
async fn test_pod_pins_to_volume_dc_and_removing_storage_clears_bindings() {
    let server = MockServer::start().await;
    volume_list(
        &server,
        "network",
        json!([{"id":"nv-1","name":"models","dataCenter":"EU-RO-1"}]),
    )
    .await;
    volume_list(&server, "global", json!([{"id":"gv-1","name":"global"}])).await;
    let mut spec = ResourceSpec::new(ResourceKind::Task, "task");
    spec.mounts = vec![
        Mount {
            path: "/models".into(),
            volume: NetworkVolume::new("models").unwrap().into(),
        },
        Mount {
            path: "/data".into(),
            volume: GlobalVolume::new("gv-1").unwrap().into(),
        },
    ];
    let mut resolver = resolver(&server);
    let mut payload =
        json!({"env":[{"key":"KEEP","value":"yes"},{"key":"RUNPOD_MOUNTS","value":"old"}]});
    attach_pod_mounts(
        &mut payload,
        &spec,
        &mut resolver,
        std::slice::from_ref(&spec),
    )
    .await
    .unwrap();
    assert_eq!(
        payload["volumeMounts"],
        json!([{"volumeId":"nv-1","volumeType":"NETWORK_VOLUME","mountPath":"/models"},{"volumeId":"gv-1","volumeType":"OBJECT_STORE_VOLUME","mountPath":"/data"}])
    );
    assert_eq!(payload["dataCenterIds"], json!(["EU-RO-1"]));
    let bindings: Value =
        serde_json::from_str(payload["env"][1]["value"].as_str().unwrap()).unwrap();
    assert_eq!(
        MountBindings::from_value(&bindings)
            .unwrap()
            .path("global", "gv-1")
            .unwrap(),
        "/data"
    );
    spec.mounts.clear();
    attach_pod_mounts(&mut payload, &spec, &mut resolver, &[])
        .await
        .unwrap();
    assert_eq!(payload["volumeMounts"], json!([]));
    assert_eq!(
        payload["env"],
        json!([{"key":"KEEP","value":"yes"},{"key":"RUNPOD_MOUNTS","value":"[]"}])
    );
}

#[tokio::test]
async fn test_global_attachment_does_not_pin_datacenter() {
    let server = MockServer::start().await;
    volume_list(&server, "global", json!([{"id":"gv-1","name":"models"}])).await;
    let mut spec = ResourceSpec::new(ResourceKind::Queue, "queue");
    spec.mounts = vec![Mount {
        path: "/runpod-volume".into(),
        volume: GlobalVolume::new("models").unwrap().into(),
    }];
    let mut payload = json!({"locations":"EU-RO-1,US-KS-2"});
    let mut resolver = resolver(&server);
    attach_endpoint_volumes(
        &mut payload,
        &spec,
        &mut resolver,
        std::slice::from_ref(&spec),
    )
    .await
    .unwrap();
    assert_eq!(payload["locations"], "EU-RO-1,US-KS-2");
    assert_eq!(payload["networkVolumeIds"], json!([]));
    assert_eq!(
        payload["volumes"],
        json!([{"volumeId":"gv-1","volumeType":"OBJECT_STORE_VOLUME"}])
    );
    spec.mounts.clear();
    attach_endpoint_volumes(&mut payload, &spec, &mut resolver, &[])
        .await
        .unwrap();
    assert_eq!(payload["volumes"], json!([]));
    assert_eq!(payload["template"]["env"][0]["value"], "[]");
}

#[tokio::test]
async fn test_same_reference_in_different_backends_does_not_share_cache() {
    let server = MockServer::start().await;
    volume_list(&server, "global", json!([{"id":"gv-1","name":"models"}])).await;
    volume_list(
        &server,
        "network",
        json!([{"id":"nv-1","name":"models","dataCenter":"EU-RO-1"}]),
    )
    .await;
    let mut resolver = resolver(&server);
    assert_eq!(
        resolver
            .resolve(&GlobalVolume::new("models").unwrap().into(), &[])
            .await
            .unwrap()
            .id,
        "gv-1"
    );
    assert_eq!(
        resolver
            .resolve(&NetworkVolume::new("models").unwrap().into(), &[])
            .await
            .unwrap()
            .id,
        "nv-1"
    );
}

#[tokio::test]
async fn conflicting_pins_and_catalog_failures_cannot_create_storage() {
    let server = MockServer::start().await;
    volume_list(&server, "network", json!([])).await;
    let mut first = NetworkVolume::new("models").unwrap();
    first.datacenter = Some("EU-RO-1".into());
    let mut second = NetworkVolume::new("models").unwrap();
    second.datacenter = Some("US-KS-2".into());
    let mut spec = ResourceSpec::new(ResourceKind::Task, "work");
    spec.mounts = vec![Mount {
        path: "/models".into(),
        volume: second.into(),
    }];
    assert!(
        resolver(&server)
            .resolve(&first.clone().into(), &[spec])
            .await
            .unwrap_err()
            .to_string()
            .contains("conflicting datacenter pins: EU-RO-1, US-KS-2")
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    server.reset().await;
    volume_list(&server, "network", json!([])).await;
    Mock::given(path("/v2/catalog/datacenters"))
        .respond_with(ResponseTemplate::new(503).set_body_json(json!({"detail":"offline"})))
        .expect(1)
        .mount(&server)
        .await;
    let error = resolver(&server)
        .resolve(&first.into(), &[])
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "cannot determine network volume support for 'models': datacenter catalog lookup failed: offline"
    );
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[test]
fn deserialization_preserves_volume_reference_invariants() {
    assert!(
        serde_json::from_value::<NetworkVolume>(
            json!({"reference":"  ","size":50,"datacenter":null,"create":true})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<GlobalVolume>(json!({"reference":"","create":false})).is_err()
    );
}
