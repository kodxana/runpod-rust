use runpod::{
    Value,
    apps::{
        NativeFunction,
        entrypoint::{EntrypointRegistry, run_entrypoint},
    },
    json,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn registration_is_lazy_last_wins_and_clearing_preserves_returned_callable() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let mut registry = EntrypointRegistry::new();
    assert!(registry.get_entrypoint().is_none());
    let returned = registry.local_entrypoint(NativeFunction::sync(move |args| {
        assert!(args.args.is_empty() && args.kwargs.is_empty());
        Ok(json!(count.fetch_add(1, Ordering::SeqCst)))
    }));
    let first = registry.get_entrypoint().unwrap().clone();
    let _second = registry.local_entrypoint(NativeFunction::new(|_| async {
        tokio::task::yield_now().await;
        Ok(json!("latest"))
    }));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        run_entrypoint(registry.get_entrypoint().unwrap())
            .await
            .unwrap(),
        json!("latest")
    );
    registry.clear();
    assert!(registry.get_entrypoint().is_none());
    assert_eq!(run_entrypoint(&returned).await.unwrap(), json!(0));
    assert_eq!(run_entrypoint(&first).await.unwrap(), json!(1));
}

#[tokio::test]
async fn registries_are_isolated_and_entrypoint_errors_propagate() {
    let mut first = EntrypointRegistry::new();
    let second = EntrypointRegistry::new();
    let _entry = first.local_entrypoint(NativeFunction::new(|_| async {
        Err(runpod::Error::InvalidInput("entrypoint failed".into()))
    }));
    assert!(second.get_entrypoint().is_none());
    assert_eq!(
        run_entrypoint(first.get_entrypoint().unwrap())
            .await
            .unwrap_err()
            .to_string(),
        "entrypoint failed"
    );
    let stream =
        NativeFunction::streaming(|_| futures_util::stream::iter([Ok(json!(1)), Ok(json!(2))]));
    assert_eq!(run_entrypoint(&stream).await.unwrap(), json!([1, 2]));
}

#[test]
fn blocking_entrypoint_drives_async_work_and_returns_sync_values() {
    let asynchronous = NativeFunction::new(|_| async {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        Ok(json!(42))
    });
    assert_eq!(
        runpod::blocking::apps::run_entrypoint(&asynchronous).unwrap(),
        json!(42)
    );
    let synchronous = NativeFunction::sync(|_| Ok(Value::Null));
    assert_eq!(
        runpod::blocking::apps::run_entrypoint(&synchronous).unwrap(),
        Value::Null
    );
}

#[tokio::test]
async fn blocking_entrypoint_rejects_nested_runtime_before_execution() {
    let entry = NativeFunction::sync(|_| panic!("nested runtime must be rejected first"));
    assert!(
        runpod::blocking::apps::run_entrypoint(&entry)
            .unwrap_err()
            .to_string()
            .contains("outside a Tokio runtime")
    );
}
