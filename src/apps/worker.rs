//! Deployed queue adapter for compiled application functions.
use super::{FunctionArgs, NativeFunction, app::RegisteredFunction};
use crate::{
    Error, Result, Value,
    client::truthy,
    serverless::{ConcurrencyModifier, Handler, Worker, WorkerConfig, health::FitnessChecks},
};
use futures_util::StreamExt;
use serde_json::Map;
use std::collections::BTreeMap;

fn job_kwargs(input: Value) -> Result<Map<String, Value>> {
    let mut kwargs = match input {
        Value::Object(object) => object,
        value if !truthy(&value) => Map::new(),
        _ => {
            return Err(Error::InvalidInput(
                "queue job input must be an object".into(),
            ));
        }
    };
    kwargs.shift_remove("__empty");
    Ok(kwargs)
}

fn deployed_handler(body: NativeFunction) -> Handler {
    if body.is_streaming() {
        Handler::streaming(move |job| {
            let body = body.clone();
            async_stream::try_stream! {
                let kwargs = job_kwargs(job.input)?;
                let mut output = body.stream(FunctionArgs { args: Vec::new(), kwargs });
                while let Some(value) = output.next().await { yield value?; }
            }
        })
    } else {
        Handler::new(move |job| {
            let body = body.clone();
            async move {
                let kwargs = job_kwargs(job.input)?;
                body.call(FunctionArgs {
                    args: Vec::new(),
                    kwargs,
                })
                .await
            }
        })
    }
}

fn max_concurrency(raw: Option<&str>) -> usize {
    raw.and_then(|s| s.trim().parse().ok()).unwrap_or(1).max(1)
}

fn config_from_values(values: &BTreeMap<String, String>) -> Result<WorkerConfig> {
    let mut config = WorkerConfig::from_values(values)?;
    let maximum = max_concurrency(values.get("RUNPOD_MAX_CONCURRENCY").map(String::as_str));
    if maximum > 1 {
        config.concurrency_modifier = Some(ConcurrencyModifier::new(move |_| maximum));
    }
    Ok(config)
}

async fn initialize(resource: &RegisteredFunction) -> Result<()> {
    if let Some(hook) = &resource.init {
        hook.call(FunctionArgs::default()).await?;
    }
    Ok(())
}

async fn prepare(
    resource: RegisteredFunction,
    mut config: WorkerConfig,
) -> Result<(Handler, WorkerConfig)> {
    initialize(&resource).await?;
    config.return_aggregate_stream |= resource.body.is_streaming();
    Ok((deployed_handler(resource.body), config))
}

pub(super) async fn queue_worker(
    resource: RegisteredFunction,
    config: WorkerConfig,
) -> Result<Worker> {
    let (handler, config) = prepare(resource, config).await?;
    Worker::new(config, handler)
}

pub(super) async fn start(resource: RegisteredFunction) -> Result<()> {
    // Apps init precedes all core startup, including configuration validation and intake.
    initialize(&resource).await?;
    let mut fitness = FitnessChecks::from_env();
    let mut config = config_from_values(fitness.environment())?;
    config.return_aggregate_stream = resource.body.is_streaming();
    crate::serverless::start_with_config(deployed_handler(resource.body), &mut fitness, config)
        .await
}

#[cfg(test)]
mod tests;
