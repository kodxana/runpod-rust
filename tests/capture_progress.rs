// Tracing callsite interest is process-wide; isolate this subscriber from no-subscriber hook tests.
use runpod::serverless::{
    PrestartHooks,
    capture::{Capture, TeeWriter},
};
use std::io;
fn emit(text: &str) {
    TeeWriter::new(io::sink()).write_text(text).unwrap();
}

#[derive(Clone, Debug)]
struct CaptureSubscriber(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
impl tracing::Subscriber for CaptureSubscriber {
    fn register_callsite(
        &self,
        _: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::sometimes()
    }
    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        Some(tracing::level_filters::LevelFilter::TRACE)
    }
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Message(String);
        impl tracing::field::Visit for Message {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{value:?}");
                }
            }
        }
        let mut message = Message(String::new());
        event.record(&mut message);
        emit(&message.0);
        self.0.lock().unwrap().push(message.0);
    }
}

#[tokio::test]
async fn sdk_progress_is_forwarded_but_excluded_and_hook_dispatch_is_propagated() {
    use tracing::instrument::WithSubscriber;
    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let subscriber = CaptureSubscriber(events.clone());
    let captured = Capture::new();
    let mut hooks = PrestartHooks::new();
    hooks.register_sync_prestart_hook("load", || {
        tracing::info!("user hook diagnostic");
        Ok(())
    });
    captured
        .scope(hooks.run_prestart_phase(None))
        .with_subscriber(subscriber)
        .await
        .unwrap();
    assert_eq!(captured.text(), "user hook diagnostic");
    let events = events.lock().unwrap();
    assert!(
        events.iter().any(|e| e.contains("phase started")),
        "{events:?}"
    );
    assert!(
        events.iter().any(|e| e.contains("running hook: load")),
        "{events:?}"
    );
    assert!(events.iter().any(|e| e.contains("ready")), "{events:?}");
}
