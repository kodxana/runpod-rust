//! Wire requests and responses shared by clients and application worker runtimes.
use crate::{Result, Value};
use serde::{Deserialize, Serialize};
use serde_json::Map;

/// Argument encoding understood by the existing Apps worker protocol.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SerializationFormat {
    /// Base64 pickle values compatible with built-in Python data types.
    #[default]
    Cloudpickle,
    /// Plain JSON arguments and named keyword arguments.
    Json,
}

/// A function execution request, omitting optional fields that are absent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FunctionRequest {
    /// Function symbol to execute.
    pub function_name: String,
    /// Optional complete source text for live execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_code: Option<String>,
    /// Positional arguments encoded according to serialization_format.
    pub args: Vec<Value>,
    /// Keyword arguments encoded according to serialization_format.
    pub kwargs: Map<String, Value>,
    /// Optional application dependencies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<String>>,
    /// Optional operating system packages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_dependencies: Option<Vec<String>>,
    /// Protocol execution discriminator, normally function.
    pub execution_type: String,
    /// Whether the worker accelerates dependency/model downloads.
    pub accelerate_downloads: bool,
    /// Argument encoding selection.
    pub serialization_format: SerializationFormat,
}
impl Default for FunctionRequest {
    fn default() -> Self {
        Self {
            function_name: String::new(),
            function_code: None,
            args: Vec::new(),
            kwargs: Map::new(),
            dependencies: None,
            system_dependencies: None,
            execution_type: "function".into(),
            accelerate_downloads: true,
            serialization_format: SerializationFormat::Cloudpickle,
        }
    }
}
impl FunctionRequest {
    /// Serializes the request into its job-input object.
    ///
    /// # Errors
    /// Returns a serialization error if a value cannot be represented in JSON.
    pub fn to_input(&self) -> Result<Value> {
        Ok(serde_json::to_value(self)?)
    }
}

/// One worker response carrying either a serialized result or a JSON result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FunctionResponse {
    /// Whether execution completed successfully.
    pub success: bool,
    /// Optional base64 pickle result, taking precedence over json_result.
    pub result: Option<String>,
    /// Plain JSON result, including null.
    pub json_result: Value,
    /// Optional worker error message.
    pub error: Option<String>,
    /// Optional captured standard output.
    pub stdout: Option<String>,
}
impl FunctionResponse {
    /// Reads known response fields while preserving upstream defaults for missing fields.
    ///
    /// # Errors
    /// Returns malformed response errors for fields that have incompatible types.
    pub fn from_output(output: &Value) -> Result<Self> {
        Ok(serde_json::from_value(output.clone())?)
    }
}
