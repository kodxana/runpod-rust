use runpod::{
    json,
    utils::{
        resolve_model_cache_path_from_hugginface_repository as cache_path,
        validator::{Rule, ValueType, validate},
    },
};

#[test]
fn test_validate_success() {
    let schema = vec![
        ("x".into(), Rule::required(ValueType::Int)),
        ("a".into(), Rule::required(ValueType::Float)),
        ("w".into(), Rule::optional(ValueType::Int, json!(5))),
    ];
    assert_eq!(
        validate(json!({"x":10,"a":1}).as_object().unwrap(), &schema),
        json!({"validated_input":{"x":10,"a":1.0,"w":5}})
    );
}

#[test]
fn test_validate_constraints_error() {
    let schema = vec![(
        "x".into(),
        Rule::required(ValueType::Int).with_constraint(|v| v.as_i64().is_some_and(|v| v < 10)),
    )];
    assert_eq!(
        validate(json!({"x":10}).as_object().unwrap(), &schema),
        json!({"errors":["x does not meet the constraints."]})
    );
}

#[test]
fn test_validate_default_bypasses_constraints() {
    let schema = vec![(
        "x".into(),
        Rule::optional(ValueType::Int, json!(5)).with_constraint(|_| false),
    )];
    assert_eq!(
        validate(json!({"x":10}).as_object().unwrap(), &schema),
        json!({"validated_input":{"x":10}})
    );
}

#[test]
fn test_validate_missing_required_input_and_unexpected_input() {
    let schema = vec![("x".into(), Rule::required(ValueType::Int))];
    assert_eq!(
        validate(json!({"unexpected":"v"}).as_object().unwrap(), &schema),
        json!({"errors":["Unexpected input. unexpected is not a valid input option.","x is a required input."]})
    );
}

#[test]
fn test_validate_invalid_type() {
    let schema = vec![("x".into(), Rule::required(ValueType::Int))];
    assert_eq!(
        validate(json!({"x":"invalid"}).as_object().unwrap(), &schema),
        json!({"errors":["x should be <class 'int'> type, not <class 'str'>."]})
    );
    assert!(
        validate(json!({"x":true}).as_object().unwrap(), &schema)
            .get("errors")
            .is_none()
    );
}

#[test]
fn test_validate_none_type() {
    let schema = vec![("my_input".into(), Rule::required(ValueType::None))];
    assert!(
        validate(json!({"my_input":null}).as_object().unwrap(), &schema)
            .get("errors")
            .is_none()
    );
}

#[test]
fn test_model_cache_paths() {
    assert_eq!(
        cache_path("org/model", None).as_deref(),
        Some("/runpod/cache/org/model/main")
    );
    assert_eq!(
        cache_path("org/model:revision", None).as_deref(),
        Some("/runpod/cache/org/model/revision")
    );
    assert_eq!(
        cache_path("org/model:", None).as_deref(),
        Some("/runpod/cache/org/model/")
    );
    assert!(cache_path(":revision", None).is_none());
    assert_eq!(
        cache_path("model", Some("/data/{revision}/{model}")).as_deref(),
        Some("/data/main/model")
    );
}

#[test]
fn test_cleanup_only_named_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    for path in ["input_objects", "output_objects", "job_files", "keep"] {
        std::fs::create_dir(dir.path().join(path)).unwrap();
    }
    std::fs::write(dir.path().join("output.zip"), "test").unwrap();
    runpod::utils::clean(dir.path(), &[]).unwrap();
    assert!(dir.path().join("keep").exists());
    assert!(!dir.path().join("input_objects").exists());
    assert!(!dir.path().join("output.zip").exists());
}
