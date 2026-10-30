//! Explicit local-entrypoint registration without import-time execution or global state.
use super::{FunctionArgs, NativeFunction};
use crate::{Result, Value};

/// An owned declaration registry; the most recently registered entrypoint is selected.
///
/// Registration never executes its body. Clearing the registry leaves previously returned
/// function clones callable, and independent projects can keep independent registries.
#[derive(Debug, Default)]
pub struct EntrypointRegistry {
    entries: Vec<NativeFunction>,
}
impl EntrypointRegistry {
    /// Creates an empty declaration registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Registers a function and returns the original callable, sharing its captured state.
    pub fn local_entrypoint(&mut self, function: NativeFunction) -> NativeFunction {
        self.entries.push(function.clone());
        function
    }
    /// Borrows the most recent declaration, if any. Clone it to retain it across clearing.
    #[must_use]
    pub fn get_entrypoint(&self) -> Option<&NativeFunction> {
        self.entries.last()
    }
    /// Clears this registry without changing other project registries or retained callables.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Executes a native entrypoint with no arguments, using its existing sync/async behavior.
///
/// Synchronous bodies run on Tokio's blocking pool. Streaming native bodies are collected
/// into a JSON array, matching the native function boundary. No startup hooks run implicitly.
/// # Errors
/// Propagates body errors or a synchronous body's background-task panic/join error.
pub async fn run_entrypoint(function: &NativeFunction) -> Result<Value> {
    function.call(FunctionArgs::default()).await
}
