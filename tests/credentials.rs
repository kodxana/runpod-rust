use runpod::credentials::{CredentialStore, Overwrite};

#[test]
fn test_credentials_round_trip_and_profile_preservation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let store = CredentialStore::new(&path);
    assert!(store.get_credentials("default").unwrap().is_none());
    assert!(store.check_credentials("default").is_err());
    store
        .set_credentials("first-key", "default", Overwrite::Deny)
        .unwrap();
    store
        .set_credentials("named-key", "profile with spaces", Overwrite::Deny)
        .unwrap();
    assert_eq!(
        store.get_credentials("default").unwrap().unwrap().api_key,
        "first-key"
    );
    assert!(
        store
            .set_credentials("other", "default", Overwrite::Deny)
            .is_err()
    );
    store
        .set_credentials("updated", "default", Overwrite::Allow)
        .unwrap();
    assert_eq!(
        store
            .get_credentials("profile with spaces")
            .unwrap()
            .unwrap()
            .api_key,
        "named-key"
    );
    assert!(!format!("{:?}", store.get_credentials("default").unwrap()).contains("updated"));
    std::fs::write(
        &path,
        format!(
            "# retain this comment\n{}",
            std::fs::read_to_string(&path).unwrap()
        ),
    )
    .unwrap();
    store
        .set_credentials("new-key", "default", Overwrite::Allow)
        .unwrap();
    assert!(
        std::fs::read_to_string(path)
            .unwrap()
            .starts_with("# retain this comment")
    );
}

#[test]
fn test_credentials_invalid_toml_is_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[invalid").unwrap();
    let store = CredentialStore::new(&path);
    assert!(store.get_credentials("default").unwrap().is_none());
    assert!(
        store
            .set_credentials("key", "default", Overwrite::Allow)
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "[invalid");
}
