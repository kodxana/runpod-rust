use super::{
    Handler, HandlerKind, JobsProgress, PrestartHooks, ResultKind, WorkerConfig, WorkerJob,
    WorkerTransport,
    handler::{handler_error, normalize_output},
};
use crate::{Error, Result, Value, client::truthy, json};
use futures_util::{
    FutureExt, StreamExt,
    future::{AbortHandle, Abortable, BoxFuture},
    stream::FuturesUnordered,
};
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    time::Duration,
};

/// A concurrent native worker with startup gating, cancellation, and heartbeat support.
#[derive(Debug)]
pub struct Worker {
    transport: WorkerTransport,
    handler: Handler,
    hooks: PrestartHooks,
    progress: JobsProgress,
}

struct ProgressGuard(JobsProgress);
impl Drop for ProgressGuard {
    fn drop(&mut self) {
        self.0.clear();
    }
}

impl Worker {
    /// Creates a worker from an owned handler and platform configuration.
    ///
    /// # Errors
    /// Returns invalid worker configuration or HTTP initialization errors.
    pub fn new(config: WorkerConfig, handler: Handler) -> Result<Self> {
        Ok(Self {
            transport: WorkerTransport::new(config)?,
            handler,
            hooks: PrestartHooks::new(),
            progress: JobsProgress::new(),
        })
    }
    /// Returns the registry for adding startup hooks before starting the worker.
    pub fn prestart_hooks_mut(&mut self) -> &mut PrestartHooks {
        &mut self.hooks
    }
    /// Returns a shared snapshot interface for queued and active jobs.
    pub fn progress(&self) -> JobsProgress {
        self.progress.clone()
    }
    /// Runs one local job after startup hooks; no webhook is contacted.
    ///
    /// # Errors
    /// Returns startup failure, missing local input, or handler stream errors.
    pub async fn run_local(&self, mut job: Value) -> Result<Value> {
        if job.get("input").is_none_or(Value::is_null) {
            return Err(Error::InvalidInput(
                "Job has no input parameter. Unable to run.".into(),
            ));
        }
        if job.get("id").is_none() {
            job["id"] = json!("local_test");
        }
        let job: WorkerJob = serde_json::from_value(job)?;
        self.hooks
            .run_prestart_phase(self.transport.config().prestart_timeout)
            .await
            .map_err(|e| Error::InvalidInput(e.to_string()))?;
        let mut stream = self.handler.run(job);
        if self.handler.kind() == HandlerKind::Single {
            return Ok(match stream.next().await {
                Some(Ok(value)) => normalize_output(value),
                Some(Err(error)) => handler_error(&error),
                None => normalize_output(Value::Null),
            });
        }
        let mut output = Vec::new();
        while let Some(item) = stream.next().await {
            output.push(item?);
        }
        Ok(json!({"output":output}))
    }
    /// Runs until shutdown resolves, then stops intake and drains active handlers.
    ///
    /// Startup hooks run concurrently with bounded intake. No handler runs before they
    /// finish. Stop-channel signals cancel individual handler futures. Dropping this
    /// future cancels all work and clears the job tracker.
    ///
    /// # Errors
    /// Returns startup failures or final-result delivery failures.
    pub async fn run(&self, shutdown: impl Future<Output = ()>) -> Result<()> {
        let _guard = ProgressGuard(self.progress.clone());
        let config = self.transport.config();
        let mut queue = VecDeque::<WorkerJob>::new();
        let mut running = FuturesUnordered::new();
        let mut aborts = HashMap::<String, AbortHandle>::new();
        let mut take: Option<BoxFuture<'_, Result<Vec<WorkerJob>>>> = None;
        let mut stop = self.poll_stops(Duration::ZERO).boxed();
        let mut prestart = self
            .hooks
            .run_prestart_phase(config.prestart_timeout)
            .boxed();
        let mut prestart_done = false;
        let mut shutting_down = false;
        let shutdown = shutdown.fuse();
        let pings = self.ping_loop().fuse();
        futures_util::pin_mut!(shutdown, pings);
        loop {
            if prestart_done {
                while running.len() < config.concurrency {
                    let Some(job) = queue.pop_front() else {
                        break;
                    };
                    let id = job.id.clone();
                    let (abort, registration) = AbortHandle::new_pair();
                    aborts.insert(id.clone(), abort);
                    running.push(async move {
                        (id, Abortable::new(self.handle_job(job), registration).await)
                    });
                }
            }
            if shutting_down && running.is_empty() && queue.is_empty() {
                return Ok(());
            }
            let free = config
                .concurrency
                .saturating_sub(running.len() + queue.len());
            if !shutting_down && free > 0 && take.is_none() {
                take = Some(self.transport.get_job(free, &self.progress).boxed());
            }
            tokio::select! {
                _ = &mut shutdown, if !shutting_down => {
                    shutting_down=true;
                    take=None;
                    if !prestart_done {
                        for job in queue.drain(..) {
                            let payload=json!({"error":json!({"event":"prestart_cancelled","error_type":"PrestartCancelled","error_message":"worker shutdown began before prestart completed","worker_id":config.worker_id,"runpod_version":env!("CARGO_PKG_VERSION")}).to_string()});
                            self.transport.send_result(&payload,&job,ResultKind::Single).await?;
                            self.progress.remove(&job.id);
                        }
                    }
                },
                result = &mut prestart, if !prestart_done && !shutting_down => {
                    prestart_done=true;
                    if let Err(error)=result {
                        if queue.is_empty()
                            && let Some(pending)=take.take()
                            && let Ok(Ok(jobs))=tokio::time::timeout(Duration::from_secs(10),pending).await {
                            queue.extend(jobs);
                        }
                        let payload=json!({"error":json!({"event":"prestart_failed","error_type":"PrestartError","error_message":error.to_string(),"worker_id":config.worker_id,"runpod_version":env!("CARGO_PKG_VERSION")}).to_string()});
                        for job in queue.drain(..) {self.transport.send_result(&payload,&job,ResultKind::Single).await?;}
                        return Err(Error::InvalidInput(error.to_string()));
                    }
                },
                result = async {match take.as_mut() {Some(future)=>future.await,None=>std::future::pending().await}}, if take.is_some() => {
                    take=None;
                    let delay=match result {
                        Ok(jobs) if !jobs.is_empty()=> {
                            for job in jobs {
                                if self.progress.get(&job.id).is_none() {self.progress.add(job.clone());queue.push_back(job);}
                            }
                            Duration::ZERO
                        },
                        Ok(_)=>config.idle_interval,
                        Err(error)=> {
                            tracing::warn!(%error,"job intake failed");
                            if error.status_code()==Some(429) {Duration::from_secs(5)} else {config.idle_interval}
                        }
                    };
                    let free=config.concurrency.saturating_sub(running.len()+queue.len());
                    if free>0 && !delay.is_zero() {
                        take=Some(async move {tokio::time::sleep(delay).await;self.transport.get_job(free,&self.progress).await}.boxed());
                    }
                },
                Some((id,result)) = running.next(), if !running.is_empty() => {
                    self.progress.remove(&id); aborts.remove(&id);
                    if let Ok(result)=result {result?;}
                    if config.refresh_worker {shutting_down=true;take=None;}
                },
                result = &mut stop, if !shutting_down => {
                    let mut delay=config.idle_interval;
                    match result {
                        Ok(ids)=>for id in ids {
                            if let Some(handle)=aborts.get(&id) {handle.abort();}
                            else if let Some(index)=queue.iter().position(|job|job.id==id) {queue.remove(index);self.progress.remove(&id);}
                        },
                        Err(error)=>{tracing::warn!(%error,"stop channel failed");if error.status_code()==Some(429) {delay=Duration::from_secs(5);}}
                    }
                    stop=self.poll_stops(delay).boxed();
                },
                _ = &mut pings => {},
            }
        }
    }
    async fn poll_stops(&self, delay: Duration) -> Result<Vec<String>> {
        if self.transport.job_stop_url().is_none() {
            return std::future::pending().await;
        }
        tokio::time::sleep(delay).await;
        self.transport.get_stop_signals().await
    }
    async fn ping_loop(&self) {
        loop {
            if let Err(error) = self.transport.ping(&self.progress).await {
                tracing::warn!(%error,"worker heartbeat failed");
            }
            tokio::time::sleep(self.transport.config().ping_interval).await;
        }
    }
    /// Processes and delivers a single job using this worker's result protocol.
    ///
    /// # Errors
    /// Returns output delivery errors. Handler errors are reported as job failures.
    pub async fn handle_job(&self, job: WorkerJob) -> Result<()> {
        let config = self.transport.config();
        let mut stream = self.handler.run(job.clone());
        let (mut result, kind) = if self.handler.kind() == HandlerKind::Single {
            (
                match stream.next().await {
                    Some(Ok(value)) => normalize_output(value),
                    Some(Err(error)) => handler_error(&error),
                    None => normalize_output(Value::Null),
                },
                ResultKind::Single,
            )
        } else {
            let mut output = Vec::new();
            let mut failure = None;
            while let Some(value) = stream.next().await {
                match value {
                    Err(error) => {
                        failure = Some(handler_error(&error));
                        break;
                    }
                    Ok(value) => {
                        if let Some(error) = value.get("error").filter(|v| truthy(v)) {
                            failure = Some(
                                json!({"error":error.as_str().map(str::to_owned).unwrap_or_else(||error.to_string())}),
                            );
                            break;
                        }
                        self.transport
                            .stream_result(&json!({"output":value}), &job)
                            .await?;
                        if config.return_aggregate_stream {
                            output.push(value);
                        }
                    }
                }
            }
            (
                failure.unwrap_or_else(|| json!({"output":output})),
                ResultKind::Stream,
            )
        };
        if config.refresh_worker {
            result["stopPod"] = json!(true);
        }
        self.transport.send_result(&result, &job, kind).await
    }
}

/// Starts a platform worker, or executes `test_input.json` in local mode.
///
/// # Errors
/// Returns invalid configuration, local input, startup, or transport errors.
pub async fn start(handler: Handler) -> Result<()> {
    let config = WorkerConfig::from_env()?;
    let local = config.job_get_url.is_none();
    let worker = Worker::new(config, handler)?;
    if local {
        let input = tokio::fs::read("test_input.json").await?;
        let result = worker.run_local(serde_json::from_slice(&input)?).await?;
        tracing::info!(output=%result,"local worker finished");
        if result.get("error").is_some_and(truthy) {
            return Err(Error::InvalidResponse(result["error"].to_string()));
        }
        Ok(())
    } else {
        worker
            .run(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
    }
}
