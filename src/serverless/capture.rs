//! Capture policy and Unicode-aware bounds for worker logs and startup error fields.
use std::borrow::Cow;

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
