use runpod::{Value, apps::ResourceSpec};

#[test]
fn python_resource_spec_manifests() {
    let fixtures: Vec<Value> = serde_json::from_str(include_str!("fixtures/spec.json")).unwrap();
    for fixture in fixtures {
        let spec: ResourceSpec = serde_json::from_value(fixture["kwargs"].clone()).unwrap();
        let result = spec.to_manifest();
        if let Some(error) = fixture.get("error") {
            assert_eq!(
                result.unwrap_err().to_string(),
                error.as_str().unwrap(),
                "{}",
                fixture["kwargs"]
            );
        } else {
            assert_eq!(
                result.unwrap(),
                fixture["manifest"],
                "{}",
                fixture["kwargs"]
            );
        }
    }
}
