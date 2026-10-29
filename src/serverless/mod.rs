//! Native serverless handlers, job tracking, startup hooks, and worker transport.
mod handler;
mod prestart;
mod state;
mod transport;
mod worker;

pub use handler::{Handler, HandlerKind, normalize_output, run_job};
pub use prestart::{PrestartError, PrestartHooks};
pub use state::{JobsProgress, WorkerJob};
pub use transport::{ResultKind, WorkerConfig, WorkerTransport};
pub use worker::{Worker, start};
