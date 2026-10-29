//! Native serialization for interoperable values, without executing Python pickle code.
//!
//! Python built-in values map through Serde. Python classes, executable closures, module
//! imports, and object identity have no native Rust equivalent and are not reconstructed.
use crate::{Result, Value};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Map;
use std::{fmt, path::PathBuf};

/// Explicit module source replacing Python's runtime function-source reflection.
#[derive(Clone)]
pub enum FunctionSource {
    /// Caller-provided complete module source.
    Text(String),
    /// A module file re-read for each live invocation or synchronization.
    File(PathBuf),
}
impl fmt::Debug for FunctionSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => f.debug_struct("Text").field("bytes", &text.len()).finish(),
            Self::File(path) => f.debug_tuple("File").field(path).finish(),
        }
    }
}

/// Reads explicit module source without starting an interpreter or inspecting compiled functions.
///
/// # Errors
/// Returns file access or UTF-8 errors for file sources.
pub async fn get_function_source(source: &FunctionSource) -> Result<String> {
    match source {
        FunctionSource::Text(text) => Ok(text.clone()),
        FunctionSource::File(path) => Ok(tokio::fs::read_to_string(path).await?),
    }
}

/// Encodes a Serde value as base64 Python-compatible pickle bytes.
///
/// # Errors
/// Returns errors for types unsupported by the pickle serializer.
pub fn serialize_arg<T: Serialize>(arg: &T) -> Result<String> {
    Ok(STANDARD.encode(serde_pickle::to_vec(arg, serde_pickle::SerOptions::new())?))
}

/// Encodes each positional argument independently, retaining order.
///
/// # Errors
/// Returns errors for unsupported argument values.
pub fn serialize_args(args: &[Value]) -> Result<Vec<String>> {
    args.iter().map(serialize_arg).collect()
}

/// Encodes keyword values independently while retaining the input map's order.
///
/// # Errors
/// Returns errors for unsupported argument values.
pub fn serialize_kwargs(kwargs: &Map<String, Value>) -> Result<Map<String, Value>> {
    kwargs
        .iter()
        .map(|(k, v)| Ok((k.clone(), Value::String(serialize_arg(v)?))))
        .collect()
}

/// Decodes a pickle result into a native Serde type without invoking Python code.
///
/// # Errors
/// Returns base64, unsupported pickle, or incompatible destination-type errors.
pub fn deserialize_result<T: DeserializeOwned>(result: &str) -> Result<T> {
    let bytes = STANDARD.decode(result)?;
    Ok(serde_pickle::from_slice(
        &bytes,
        serde_pickle::DeOptions::new(),
    )?)
}
