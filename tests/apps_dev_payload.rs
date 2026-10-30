use runpod::{
    Value,
    apps::dev::{DevConfig, changed_fields, comparable_payload, endpoint_input},
};

#[test]
fn python_development_payload_and_diff_fixtures() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/dev.json")).unwrap();
    for (index, case) in cases.iter().enumerate() {
        let input = &case["input"];
        let output = match case["operation"].as_str().unwrap() {
            "endpoint" => {
                let mut config = DevConfig::new("fixture-key");
                config.images.python_version = "3.12".into();
                config.images.tag = input["overrides"]["RUNPOD_RUNTIME_TAG"]
                    .as_str()
                    .unwrap_or("latest")
                    .into();
                config.image_override = input["overrides"]["RUNPOD_DEV_IMAGE"]
                    .as_str()
                    .map(str::to_owned);
                config.runtime_overrides =
                    serde_json::from_value(input["overrides"].clone()).unwrap();
                endpoint_input(
                    "demo",
                    &serde_json::from_value(input["spec"].clone()).unwrap(),
                    input["generation"].as_u64().unwrap(),
                    &config,
                    None,
                )
            }
            "comparable" => comparable_payload(input),
            "changed" => changed_fields(&input["old"], &input["new"]).map(|v| runpod::json!(v)),
            _ => panic!("unknown fixture"),
        };
        if let Some(error) = case.get("error") {
            assert_eq!(
                output.unwrap_err().to_string(),
                error.as_str().unwrap(),
                "case {index}"
            );
        } else {
            assert_eq!(output.unwrap(), case["output"], "case {index}");
        }
    }
}

#[test]
fn configuration_comparison_does_not_round_large_integers_and_rejects_malformed_input() {
    for (integer, float, equal) in [
        (9_007_199_254_740_992_u64, 9_007_199_254_740_992_f64, true),
        (9_007_199_254_740_993, 9_007_199_254_740_992_f64, false),
        (u64::MAX, 18_446_744_073_709_551_616_f64, false),
    ] {
        let fields =
            changed_fields(&runpod::json!({"n":integer}), &runpod::json!({"n":float})).unwrap();
        assert_eq!(fields.is_empty(), equal);
    }
    assert!(comparable_payload(&Value::Null).is_err());
    assert!(comparable_payload(&runpod::json!({"template":{"env":"invalid"}})).is_err());
    assert!(changed_fields(&runpod::json!({"template":3}), &runpod::json!({})).is_err());
    let mut config = DevConfig::new("hidden-api-key");
    config
        .runtime_overrides
        .insert("RUNPOD_PACKAGE_SPEC".into(), "hidden-package-token".into());
    let debug = format!("{config:?}");
    assert!(!debug.contains("hidden-api-key") && !debug.contains("hidden-package-token"));
}
