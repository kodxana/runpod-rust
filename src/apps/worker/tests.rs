use super::*;
use crate::serverless::WorkerJob;
use crate::{
    apps::{App, ResourceSpec, targets::FunctionDefinition},
    json,
    serverless::HandlerKind,
};
use futures_util::StreamExt;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn registration(body: NativeFunction, init: Option<NativeFunction>) -> RegisteredFunction {
    let app = App::new("queue-tests").unwrap();
    let handle = app
        .queue(
            ResourceSpec::default(),
            FunctionDefinition {
                name: "echo".into(),
                parameters: Vec::new(),
                source: None,
            },
            body,
        )
        .unwrap();
    if let Some(init) = init {
        handle.init(init).unwrap();
    }
    handle.registration().unwrap()
}

async fn invoke(body: NativeFunction, input: Value) -> Vec<Value> {
    let (handler, _) = prepare(registration(body, None), WorkerConfig::default())
        .await
        .unwrap();
    handler
        .run(WorkerJob::new("job", input))
        .map(Result::unwrap)
        .collect()
        .await
}

fn sequence() -> NativeFunction {
    NativeFunction::streaming(|call| {
        futures_util::stream::iter((0..call.kwargs["n"].as_u64().unwrap()).map(|i| Ok(json!(i))))
    })
}

#[tokio::test]
async fn test_sync_function() {
    let result = invoke(NativeFunction::sync(|call| {
        assert_eq!(call.args, Vec::<Value>::new());
        Ok(json!({"sum": call.kwargs["a"].as_i64().unwrap() + call.kwargs["b"].as_i64().unwrap()}))
    }), json!({"a":1,"b":2})).await;
    assert_eq!(result, [json!({"sum":3})]);
}

#[tokio::test]
async fn test_async_function() {
    assert_eq!(
        invoke(
            NativeFunction::new(|call| async move {
                tokio::task::yield_now().await;
                Ok(json!(call.kwargs["x"].as_i64().unwrap() * 2))
            }),
            json!({"x":5})
        )
        .await,
        [json!(10)]
    );
}

#[tokio::test]
async fn test_empty_marker_stripped() {
    assert_eq!(
        invoke(
            NativeFunction::sync(|call| {
                assert!(call.kwargs.is_empty());
                Ok(json!("ok"))
            }),
            json!({"__empty":true})
        )
        .await,
        [json!("ok")]
    );
}

#[tokio::test]
async fn test_no_input() {
    let job: WorkerJob = serde_json::from_value(json!({"id":"job"})).unwrap();
    assert_eq!(
        invoke(
            NativeFunction::sync(|call| {
                assert!(call.kwargs.is_empty());
                Ok(json!("ok"))
            }),
            job.input
        )
        .await,
        [json!("ok")]
    );
}

#[tokio::test]
async fn test_sync_generator() {
    let (handler, config) = prepare(registration(sequence(), None), WorkerConfig::default())
        .await
        .unwrap();
    assert_eq!(handler.kind(), HandlerKind::Streaming);
    assert!(config.return_aggregate_stream);
    assert_eq!(
        handler
            .run(WorkerJob::new("job", json!({"n":3})))
            .map(Result::unwrap)
            .collect::<Vec<_>>()
            .await,
        [json!(0), json!(1), json!(2)]
    );
}

#[tokio::test]
async fn test_async_generator() {
    let body = NativeFunction::streaming(|call| {
        async_stream::try_stream! {
            for i in 0..call.kwargs["n"].as_u64().unwrap() {
                tokio::task::yield_now().await;
                yield json!(i);
            }
        }
    });
    assert_eq!(invoke(body, json!({"n":2})).await, [json!(0), json!(1)]);
}

#[tokio::test]
async fn test_no_init() {
    prepare(registration(sequence(), None), WorkerConfig::default())
        .await
        .unwrap();
}

#[tokio::test]
async fn test_sync_init() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let hook = NativeFunction::sync(move |call| {
        assert!(call.args.is_empty() && call.kwargs.is_empty());
        observed.fetch_add(1, Ordering::SeqCst);
        Ok(Value::Null)
    });
    let (handler, _) = prepare(
        registration(sequence(), Some(hook)),
        WorkerConfig::default(),
    )
    .await
    .unwrap();
    for _ in 0..2 {
        handler
            .run(WorkerJob::new("job", json!({"n":1})))
            .collect::<Vec<_>>()
            .await;
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_async_init() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let hook = NativeFunction::new(move |_| {
        let observed = observed.clone();
        async move {
            tokio::task::yield_now().await;
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(Value::Null)
        }
    });
    prepare(
        registration(sequence(), Some(hook)),
        WorkerConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn test_default() {
    assert_eq!(max_concurrency(None), 1);
}
#[test]
fn test_env_value() {
    assert_eq!(max_concurrency(Some("8")), 8);
}
#[test]
fn test_invalid_value() {
    assert_eq!(max_concurrency(Some("banana")), 1);
}
#[test]
fn test_floor_of_one() {
    assert_eq!(max_concurrency(Some("0")), 1);
}
#[test]
fn test_worker_config_single() {
    assert!(
        config_from_values(&BTreeMap::new())
            .unwrap()
            .concurrency_modifier
            .is_none()
    );
}
#[test]
fn test_worker_config_concurrent() {
    let env = [("RUNPOD_MAX_CONCURRENCY".into(), "4".into())].into();
    assert!(
        config_from_values(&env)
            .unwrap()
            .concurrency_modifier
            .is_some()
    );
}
#[tokio::test]
async fn test_worker_config_plain_handler() {
    let (handler, config) = prepare(
        registration(NativeFunction::sync(|_| Ok(Value::Null)), None),
        WorkerConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(handler.kind(), HandlerKind::Single);
    assert!(!config.return_aggregate_stream);
}
#[tokio::test]
async fn test_worker_config_generator_handler() {
    let (_, config) = prepare(registration(sequence(), None), WorkerConfig::default())
        .await
        .unwrap();
    assert!(config.return_aggregate_stream);
}

fn fixture_function(name: &str) -> NativeFunction {
    match name {
        "echo" => NativeFunction::sync(|call| Ok(json!(call.kwargs))),
        "keys" => NativeFunction::sync(|call| Ok(json!(call.kwargs.keys().collect::<Vec<_>>()))),
        "add" => NativeFunction::sync(|call| {
            Ok(json!({"sum":call.kwargs["a"].as_i64().unwrap()+call.kwargs["b"].as_i64().unwrap()}))
        }),
        "double" => {
            NativeFunction::new(
                |call| async move { Ok(json!(call.kwargs["x"].as_i64().unwrap() * 2)) },
            )
        }
        "sequence" => sequence(),
        "async_sequence" => NativeFunction::streaming(|call| {
            async_stream::try_stream! {
                for i in 0..call.kwargs["n"].as_u64().unwrap() { tokio::task::yield_now().await; yield json!(i); }
            }
        }),
        "failure" => NativeFunction::sync(|_| Err(Error::InvalidInput("body failed".into()))),
        "stream_failure" => NativeFunction::streaming(|_| {
            futures_util::stream::iter([
                Ok(json!("first")),
                Err(Error::InvalidInput("stream failed".into())),
            ])
        }),
        _ => panic!("unknown fixture function"),
    }
}

#[tokio::test]
async fn runtime_differential_fixtures() {
    let fixtures: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/apps_queue.json")).unwrap();
    for case in fixtures["calls"].as_array().unwrap() {
        let (handler, config) = prepare(
            registration(fixture_function(case["function"].as_str().unwrap()), None),
            WorkerConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            json!(handler.kind() == HandlerKind::Streaming),
            case["streaming"]
        );
        assert_eq!(json!(config.return_aggregate_stream), case["streaming"]);
        let mut output = Vec::new();
        let mut failed = false;
        let mut stream = handler.run(WorkerJob::new("job", case["job"]["input"].clone()));
        while let Some(item) = stream.next().await {
            if let Ok(value) = item {
                output.push(value);
            } else {
                failed = true;
                break;
            }
        }
        assert_eq!(json!(output), case["outputs"], "{}", case["name"]);
        assert_eq!(json!(failed), case["error"], "{}", case["name"]);
    }
    for case in fixtures["concurrency"].as_array().unwrap() {
        let raw = case["value"].as_str();
        assert_eq!(json!(max_concurrency(raw)), case["maximum"]);
        let env = raw
            .map(|s| ("RUNPOD_MAX_CONCURRENCY".into(), s.into()))
            .into_iter()
            .collect();
        assert_eq!(
            config_from_values(&env)
                .unwrap()
                .concurrency_modifier
                .is_some(),
            !case["modified"].is_null()
        );
    }
    for case in fixtures["init"].as_array().unwrap() {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = events.clone();
        let hook = match case["kind"].as_str().unwrap() {
            "none" => None,
            "async" => Some(NativeFunction::new(move |_| {
                let observed = observed.clone();
                async move {
                    tokio::task::yield_now().await;
                    observed.lock().unwrap().push("init");
                    Ok(Value::Null)
                }
            })),
            kind => {
                let failure = kind == "failure";
                Some(NativeFunction::sync(move |_| {
                    observed.lock().unwrap().push("init");
                    if failure {
                        Err(Error::InvalidInput("init failed".into()))
                    } else {
                        Ok(Value::Null)
                    }
                }))
            }
        };
        let result = prepare(registration(sequence(), hook), WorkerConfig::default()).await;
        assert_eq!(json!(result.is_err()), case["error"]);
        assert_eq!(json!(*events.lock().unwrap()), case["events"]);
    }
}

#[test]
fn typed_input_and_sized_concurrency_adaptations() {
    assert!(
        job_kwargs(json!([["x", 1]])).is_err(),
        "native kwargs require an object"
    );
    assert_eq!(
        max_concurrency(Some("99999999999999999999999999999999999999")),
        1
    );
    assert_eq!(max_concurrency(Some("1_000")), 1);
}
