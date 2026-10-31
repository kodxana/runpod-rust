//! Native serverless handlers, job tracking, startup hooks, and worker transport.
pub mod capture;
mod handler;
pub mod health;
mod prestart;
mod scaling;
mod state;
mod transport;
mod worker;

pub use handler::{
    Handler, HandlerKind, normalize_output, run_job, run_job_generator,
    run_job_generator_with_capture, run_job_with_capture,
};
pub use prestart::{
    PrestartError, PrestartHooks, build_prestart_cancelled_payload, build_prestart_failed_payload,
};
pub use scaling::{ConcurrencyModifier, WorkerControl};
pub use state::{JobsProgress, WorkerJob};
pub use transport::{ResultKind, WorkerConfig, WorkerTransport};
pub use worker::{Worker, start, start_with_fitness};
