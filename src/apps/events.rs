//! Optional observation callbacks whose returned errors do not fail SDK operations.
use crate::{Result, Value};
use std::{fmt, sync::Arc};

type Handler = dyn Fn(&str, &[Value]) -> Result<()> + Send + Sync;
/// A named-event callback shared by clones and owned by a provisioning context.
#[derive(Clone)]
pub struct EventSink(Arc<Handler>);
impl fmt::Debug for EventSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventSink").finish_non_exhaustive()
    }
}
impl EventSink {
    /// Registers a callback; ignore unhandled event names in the callback itself.
    pub fn new(handler: impl Fn(&str, &[Value]) -> Result<()> + Send + Sync + 'static) -> Self {
        Self(Arc::new(handler))
    }
}

/// Emits an event when a sink exists, logging and suppressing callback errors.
///
/// Rust panics retain their normal behavior; callbacks should return errors on failure.
pub fn emit(sink: Option<&EventSink>, event: &str, args: &[Value]) {
    if let Some(sink) = sink
        && let Err(error) = (sink.0)(event, args)
    {
        tracing::debug!(event,%error,"event sink failed");
    }
}
