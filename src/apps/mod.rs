//! Application resource definitions and provisioning operations.
pub mod catalog;
pub mod gpu;
pub mod graphql;
mod model;
mod secret;
pub mod spec;
pub mod volume;
pub use catalog::{CpuInstanceType, DataCenter, GpuGroup, GpuType};
pub use model::Model;
pub use secret::{EnvValue, Secret, render_env, secret_names};
pub use spec::{ResourceKind, ResourceSpec, RouteSpec, Selection, Workers};
pub use volume::{GlobalVolume, Mount, NetworkVolume, Volume};
