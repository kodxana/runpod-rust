//! Rust clients and workers for Runpod, ported from the Python SDK.
//!
//! Clients own their credentials and connection pools; there is no mutable global API key.
//! All network operations are asynchronous unless accessed through [`blocking`].
//!
//! ```no_run
//! use runpod::{Client, json};
//! use std::time::Duration;
//!
//! # async fn example() -> runpod::Result<()> {
//! let client = Client::from_env()?;
//! let mut job = client.endpoint("ENDPOINT_ID").run(json!({"prompt":"hello"})).await?;
//! let output = job.output(Duration::from_secs(60)).await?;
//! # let _ = output;
//! # Ok(())
//! # }
//! ```

pub mod apps;
pub mod blocking;
pub mod client;
pub mod control;
pub mod credentials;
pub mod endpoint;
pub mod error;
mod http_docs;
pub mod serverless;
pub mod utils;

pub use client::{Client, ClientConfig};
pub use endpoint::{AsyncioEndpoint, AsyncioJob, Endpoint, Job};
pub use error::{Error, Result};
pub use serde_json::{Value, json};
