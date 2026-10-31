use runpod::{
    Error,
    serverless::{PrestartError, PrestartHooks},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[tokio::test]
async fn synchronous_and_async_hooks_run_in_order_on_their_intended_threads() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let runtime_thread = std::thread::current().id();
    let mut hooks = PrestartHooks::new();
    let calls = events.clone();
    hooks.register_sync_prestart_hook("sync", move || {
        assert_ne!(std::thread::current().id(), runtime_thread);
        assert_eq!(std::thread::current().name(), Some("rp-prestart"));
        calls.lock().unwrap().push(1);
        Ok(())
    });
    let calls = events.clone();
    hooks.register_prestart_hook("async", move || {
        let calls = calls.clone();
        async move {
            assert_eq!(std::thread::current().id(), runtime_thread);
            calls.lock().unwrap().push(2);
            Ok(())
        }
    });
    hooks
        .run_prestart_hooks_async(Some(Duration::from_secs(1)))
        .await
        .unwrap();
    assert_eq!(*events.lock().unwrap(), [1, 2]);
}

#[tokio::test]
async fn blocked_sync_hook_times_out_without_blocking_the_runtime() {
    let (release, wait) = std::sync::mpsc::channel();
    let wait = Arc::new(Mutex::new(wait));
    let (finished, done) = tokio::sync::oneshot::channel();
    let finished = Mutex::new(Some(finished));
    let mut hooks = PrestartHooks::new();
    hooks.register_sync_prestart_hook("blocked", move || {
        wait.lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        if let Some(sender) = finished.lock().unwrap().take() {
            let _ = sender.send(());
        }
        Ok(())
    });
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        hooks.run_prestart_phase(Some(Duration::from_millis(20))),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(error,PrestartError::Timeout{ref hook,..} if hook=="blocked"));
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), done)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn zero_timeout_never_invokes_a_hook_or_factory() {
    let invoked = Arc::new(AtomicBool::new(false));
    let seen = invoked.clone();
    let mut hooks = PrestartHooks::new();
    hooks.register_prestart_hook("never", move || {
        seen.store(true, Ordering::SeqCst);
        async { Ok(()) }
    });
    assert!(matches!(
        hooks.run_prestart_phase(Some(Duration::ZERO)).await,
        Err(PrestartError::Timeout { ref hook, .. }) if hook == "unknown"
    ));
    assert!(!invoked.load(Ordering::SeqCst));
    let empty = PrestartHooks::new();
    empty
        .run_prestart_phase(Some(Duration::ZERO))
        .await
        .unwrap();
    assert!(matches!(
        empty.run_prestart_hooks_async(Some(Duration::ZERO)).await,
        Err(PrestartError::Timeout { .. })
    ));
}

#[tokio::test(start_paused = true)]
async fn one_deadline_covers_all_hooks_and_preserves_hook_errors() {
    let mut hooks = PrestartHooks::new();
    for name in ["first", "second"] {
        hooks.register_prestart_hook(name, || async {
            tokio::time::sleep(Duration::from_millis(60)).await;
            Ok(())
        });
    }
    let error = hooks
        .run_prestart_phase(Some(Duration::from_millis(100)))
        .await
        .unwrap_err();
    assert!(
        matches!(error,PrestartError::Timeout{ref hook,timeout} if hook=="second" && timeout==Duration::from_millis(100))
    );
    hooks.clear_prestart_hooks();
    hooks.register_prestart_hook("backend", || async { Err(Error::Timeout) });
    let error = hooks
        .run_prestart_phase(Some(Duration::from_secs(1)))
        .await
        .unwrap_err();
    assert!(
        matches!(error,PrestartError::Failed{ref hook,ref source} if hook=="backend" && matches!(**source,Error::Timeout))
    );
}

#[tokio::test]
async fn sync_failure_retains_cause_and_prevents_later_hooks() {
    let mut hooks = PrestartHooks::new();
    hooks.register_sync_prestart_hook("load", || Err(Error::InvalidInput("bad model size".into())));
    hooks.register_prestart_hook("unreached", || async { panic!("later hook ran") });
    let error = hooks.run_prestart_phase(None).await.unwrap_err();
    assert!(
        matches!(error,PrestartError::Failed{ref hook,ref source} if hook=="load" && matches!(**source,Error::InvalidInput(ref message) if message=="bad model size"))
    );
    assert!(std::error::Error::source(&error).is_some());
}
