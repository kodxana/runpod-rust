use runpod::serverless::{
    PrestartHooks,
    capture::{Capture, TeeWriter, paused, paused_sync},
};
use std::io;

fn emit(text: &str) {
    TeeWriter::new(io::sink()).write_text(text).unwrap();
}

#[test]
fn records_and_passes_through() {
    let captured = Capture::new();
    let mut writer = TeeWriter::new(Vec::new());
    captured.scope_sync(|| {
        assert_eq!(writer.write_text("hello \u{1f680}\n").unwrap(), 8);
        writer.flush().unwrap();
    });
    assert_eq!(captured.text(), "hello \u{1f680}\n");
    assert_eq!(writer.into_inner(), "hello \u{1f680}\n".as_bytes());
}

#[test]
fn writelines_is_captured_and_outer_scope_is_restored() {
    let outer = Capture::new();
    let inner = Capture::new();
    let mut writer = TeeWriter::new(Vec::new());
    outer.scope_sync(|| {
        writer.write_text("before\n").unwrap();
        inner.scope_sync(|| writer.write_lines(["a\n", "b\n"]).unwrap());
        writer.write_text("after\n").unwrap();
    });
    writer.write_text("outside\n").unwrap();
    assert_eq!(outer.text(), "before\nafter\n");
    assert_eq!(inner.text(), "a\nb\n");
    assert_eq!(writer.into_inner(), b"before\na\nb\nafter\noutside\n");
    assert!(Capture::current().is_none());
}

#[tokio::test]
async fn pause_restores_capture_across_await_and_nested_sync_calls() {
    let captured = Capture::new();
    captured
        .scope(async {
            emit("before");
            paused(async {
                emit("hidden async");
                tokio::task::yield_now().await;
                paused_sync(|| emit("hidden sync"));
            })
            .await;
            emit("after");
        })
        .await;
    assert_eq!(captured.text(), "beforeafter");
    assert!(Capture::current().is_none());
}

#[tokio::test]
async fn concurrent_futures_on_one_thread_have_isolated_buffers() {
    let first = Capture::new();
    let second = Capture::new();
    let write = |text: &'static str| async move {
        for _ in 0..3 {
            emit(text);
            tokio::task::yield_now().await;
        }
    };
    tokio::join!(first.scope(write("A")), second.scope(write("B")));
    assert_eq!(first.text(), "AAA");
    assert_eq!(second.text(), "BBB");
}

#[tokio::test]
async fn cancellation_and_unwinding_restore_the_context() {
    let captured = Capture::new();
    let owned = captured.clone();
    let (started, ready) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        owned
            .scope(async {
                emit("before cancellation");
                started.send(()).unwrap();
                std::future::pending::<()>().await;
            })
            .await;
    });
    ready.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    emit("outside");
    assert_eq!(captured.text(), "before cancellation");
    let panic = std::panic::catch_unwind(|| captured.scope_sync(|| panic!("scope unwind")));
    assert!(panic.is_err());
    assert!(Capture::current().is_none());
}

#[tokio::test]
async fn blocking_hook_inherits_capture_and_user_threads_can_copy_it() {
    let captured = Capture::new();
    let mut hooks = PrestartHooks::new();
    hooks.register_sync_prestart_hook("load", || {
        emit("blocking ");
        Ok(())
    });
    captured
        .scope(async {
            hooks.run_prestart_phase(None).await.unwrap();
            let context = Capture::current().unwrap();
            tokio::task::spawn_blocking(move || context.scope_sync(|| emit("copied")))
                .await
                .unwrap();
        })
        .await;
    assert_eq!(captured.text(), "blocking copied");
}

#[test]
fn forwarding_errors_remain_typed_and_do_not_mirror_failed_writes() {
    struct Broken;
    impl io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::PermissionDenied.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }
    let captured = Capture::new();
    let mut writer = TeeWriter::new(Broken);
    captured.scope_sync(|| {
        assert_eq!(
            writer.write_text("unwritten").unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            writer.flush().unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    });
    assert!(captured.text().is_empty());
}
