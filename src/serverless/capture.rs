//! Capture policy and Unicode-aware bounds for worker logs and startup error fields.
use std::{
    borrow::Cow,
    fmt,
    future::Future,
    io,
    sync::{Arc, Mutex},
};

tokio::task_local! {
    static CURRENT: Option<Capture>;
}

/// Shared output tail whose scopes follow future polling rather than executor threads.
/// Output must use [`TeeWriter`]; ordinary `println!` and external process output are not intercepted.
#[derive(Clone, Default)]
pub struct Capture {
    buffer: Arc<Mutex<RingBuffer>>,
}

impl fmt::Debug for Capture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Capture").finish_non_exhaustive()
    }
}

impl Capture {
    /// Create an empty shared buffer with the default captured-character limit.
    pub fn new() -> Self {
        Self::default()
    }

    /// Copy the active context for explicit propagation into a new thread or spawned task.
    /// Tokio task spawning does not inherit this context automatically.
    pub fn current() -> Option<Self> {
        CURRENT.try_with(Clone::clone).ok().flatten()
    }

    /// Capture writes while polling a future, restoring the outer context on yield or cancellation.
    pub async fn scope<F: Future>(&self, future: F) -> F::Output {
        CURRENT.scope(Some(self.clone()), future).await
    }

    /// Capture writes during a synchronous call, restoring the context even if it unwinds.
    pub fn scope_sync<T>(&self, call: impl FnOnce() -> T) -> T {
        CURRENT.sync_scope(Some(self.clone()), call)
    }

    /// Snapshot the retained output; an unavailable capture buffer is treated as empty.
    pub fn text(&self) -> String {
        self.buffer
            .lock()
            .map(|buffer| buffer.as_str().to_owned())
            .unwrap_or_default()
    }

    fn record(&self, text: &str) {
        // Capture is best effort and must never turn successful output into an I/O failure.
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.write(text);
        }
    }
}

/// Poll a future without mirroring its output, preserving any enclosing capture scope.
pub async fn paused<F: Future>(future: F) -> F::Output {
    CURRENT.scope(None, future).await
}

/// Run a synchronous diagnostic without mirroring its output into the active capture.
pub fn paused_sync<T>(call: impl FnOnce() -> T) -> T {
    CURRENT.sync_scope(None, call)
}

/// Forward UTF-8 text to an owned writer and mirror successful writes into the active capture.
/// This text interface counts characters; the underlying writer still receives UTF-8 bytes.
#[derive(Debug)]
pub struct TeeWriter<W> {
    inner: W,
}

impl<W: io::Write> TeeWriter<W> {
    /// Wrap a destination without installing or changing process-wide output handles.
    pub fn new(inner: W) -> Self {
        Self { inner }
    }

    /// Forward all text, then capture it, returning the number of Unicode scalar values written.
    ///
    /// # Errors
    /// Returns the destination's I/O error unchanged; a failed write is not mirrored.
    pub fn write_text(&mut self, text: &str) -> io::Result<usize> {
        self.inner.write_all(text.as_bytes())?;
        if let Some(captured) = Capture::current() {
            captured.record(text);
        }
        Ok(text.chars().count())
    }

    /// Forward and capture lines in order without adding line terminators.
    ///
    /// # Errors
    /// Returns the first destination write error, preserving previously written lines.
    pub fn write_lines<I, S>(&mut self, lines: I) -> io::Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for line in lines {
            self.write_text(line.as_ref())?;
        }
        Ok(())
    }

    /// Flush the underlying writer.
    ///
    /// # Errors
    /// Returns the destination's flush error unchanged.
    pub fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }

    /// Borrow the destination for operations outside the text interface.
    pub fn inner(&self) -> &W {
        &self.inner
    }

    /// Mutably borrow the destination; direct writes bypass capture.
    pub fn inner_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    /// Recover the owned destination without implicitly flushing it.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

/// Open a text writer forwarding to standard output and the active capture scope.
pub fn stdout() -> TeeWriter<io::Stdout> {
    TeeWriter::new(io::stdout())
}

/// Open a text writer forwarding to standard error and the active capture scope.
pub fn stderr() -> TeeWriter<io::Stderr> {
    TeeWriter::new(io::stderr())
}

/// Maximum number of Unicode scalar values retained in captured output by default.
pub const MAX_CAPTURED_CHARS: usize = 16 * 1024;

/// Select which worker failures may include captured output.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    /// Capture startup output when hooks are registered, excluding handler output.
    #[default]
    Auto,
    /// Capture startup and handler output.
    All,
    /// Do not capture output.
    Off,
}

impl CaptureMode {
    /// Read `RUNPOD_LOG_CAPTURE`, defaulting to automatic capture for missing or unknown values.
    pub fn from_env() -> Self {
        std::env::var("RUNPOD_LOG_CAPTURE")
            .map(|value| Self::from(value.as_str()))
            .unwrap_or_default()
    }

    /// Whether handler failure logs have been explicitly enabled.
    pub fn handler_capture_enabled(self) -> bool {
        self == Self::All
    }
}

impl From<&str> for CaptureMode {
    /// Parse a case-insensitive mode, trimming whitespace and defaulting unknown values to auto.
    fn from(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "all" => Self::All,
            "off" => Self::Off,
            _ => Self::Auto,
        }
    }
}

/// Text buffer retaining the most recent Unicode scalar values across writes.
#[derive(Debug, Clone)]
pub struct RingBuffer {
    limit: usize,
    text: String,
}

impl Default for RingBuffer {
    fn default() -> Self {
        Self::new(MAX_CAPTURED_CHARS)
    }
}

impl RingBuffer {
    /// Create an empty buffer with a character limit; zero retains all text, matching Python.
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            text: String::new(),
        }
    }

    /// Append text and return its character count, discarding the oldest excess characters.
    pub fn write(&mut self, text: &str) -> usize {
        if self.limit == 0 {
            self.text.push_str(text);
        } else if let Some((start, _)) = text.char_indices().rev().nth(self.limit - 1) {
            // An oversized write must not leave its entire allocation resident in the buffer.
            self.text.clear();
            self.text.push_str(&text[start..]);
        } else {
            self.text.push_str(text);
            if let Some((start, _)) = self.text.char_indices().rev().nth(self.limit - 1) {
                self.text.drain(..start);
            }
        }
        text.chars().count()
    }

    /// Borrow the retained text in its original write order.
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

/// Keep a long error's head and tail, adding the number of omitted characters between them.
/// The marker is additional to `limit`. Limits zero and one retain the full tail, matching Python.
/// Short text is borrowed without allocating. Limits count Unicode scalar values, not bytes.
pub fn clip(text: &str, limit: usize) -> Cow<'_, str> {
    let length = text.chars().count();
    if length <= limit {
        return Cow::Borrowed(text);
    }
    let keep = limit / 2;
    let head_end = text.char_indices().nth(keep).map_or(text.len(), |(i, _)| i);
    // Python's [-0:] slice retains the complete string.
    let tail_start = if keep == 0 {
        0
    } else {
        text.char_indices()
            .rev()
            .nth(keep - 1)
            .map_or(0, |(i, _)| i)
    };
    let omitted = length - 2 * keep;
    Cow::Owned(format!(
        "{}\n...[{omitted} characters truncated]...\n{}",
        &text[..head_end],
        &text[tail_start..]
    ))
}
