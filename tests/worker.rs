use runpod::{
    Value, json,
    serverless::{Handler, JobsProgress, PrestartHooks, WorkerJob, run_job},
};
use std::time::Duration;

#[tokio::test]
async fn python_run_job_envelopes() {
    let fixtures: Vec<Value> = serde_json::from_str(include_str!("fixtures/worker.json")).unwrap();
    for fixture in fixtures {
        let output = fixture["handler_output"].clone();
        let result = run_job(WorkerJob::new("job", json!({})), move |_| async move {
            Ok(output)
        })
        .await;
        assert_eq!(
            result, fixture["result"],
            "handler output {}",
            fixture["handler_output"]
        );
    }
}

#[tokio::test]
async fn test_run_job_async_and_error() {
    let result = run_job(WorkerJob::new("id", json!({"value":3})), |job| async move {
        Ok(job.input)
    })
    .await;
    assert_eq!(result, json!({"output":{"value":3}}));
    let result = run_job(WorkerJob::new("id", Value::Null), |_| async {
        Err(runpod::Error::InvalidInput("handler failed".into()))
    })
    .await;
    let error: Value = serde_json::from_str(result["error"].as_str().unwrap()).unwrap();
    assert_eq!(error["error_message"], "handler failed");
}

#[test]
fn test_jobs_progress() {
    let jobs = JobsProgress::new();
    assert!(jobs.get_job_list().is_none());
    jobs.add(WorkerJob::new("job", json!(1)));
    jobs.add(WorkerJob::new("job", json!(2)));
    assert_eq!(jobs.get_job_count(), 1);
    assert_eq!(jobs.get("job").unwrap().input, json!(1));
    let cloned = jobs.clone();
    cloned.remove("job");
    assert!(jobs.is_empty());
    jobs.remove("missing");
}

#[tokio::test]
async fn test_prestart_registration_order_and_timeout() {
    use std::sync::{Arc, Mutex};
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut hooks = PrestartHooks::new();
    for i in 0..3 {
        let calls = calls.clone();
        hooks.register_prestart_hook(format!("hook{i}"), move || {
            let calls = calls.clone();
            async move {
                calls.lock().unwrap().push(i);
                Ok(())
            }
        });
    }
    hooks
        .run_prestart_phase(Some(Duration::from_secs(1)))
        .await
        .unwrap();
    assert_eq!(*calls.lock().unwrap(), vec![0, 1, 2]);
    hooks.clear_prestart_hooks();
    assert!(!hooks.has_prestart_hooks());
    hooks.register_prestart_hook("slow", || async {
        tokio::time::sleep(Duration::from_secs(60)).await;
        Ok(())
    });
    assert!(
        hooks
            .run_prestart_phase(Some(Duration::from_millis(5)))
            .await
            .unwrap_err()
            .to_string()
            .contains("slow")
    );
}

#[tokio::test]
async fn test_stream_generator() {
    use futures_util::StreamExt;
    let handler =
        Handler::streaming(|_| futures_util::stream::iter(vec![Ok(json!(1)), Ok(json!(2))]));
    let chunks: Vec<_> = handler.run(WorkerJob::new("id", json!({}))).collect().await;
    assert_eq!(
        chunks
            .into_iter()
            .collect::<runpod::Result<Vec<_>>>()
            .unwrap(),
        vec![json!(1), json!(2)]
    );
}
