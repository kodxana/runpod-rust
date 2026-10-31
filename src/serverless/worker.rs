use super::{
    Handler, HandlerKind, JobsProgress, PrestartHooks, ResultKind, WorkerConfig, WorkerControl,
    WorkerJob, WorkerTransport,
    handler::{handler_error, normalize_output},
    health::FitnessChecks,
    scaling::IntakeSchedule,
};
use crate::{Error, Result, Value, client::truthy, json};
use futures_util::{
    FutureExt, StreamExt,
    future::{AbortHandle, Abortable, BoxFuture},
    stream::FuturesUnordered,
};
use std::{collections::VecDeque, future::Future, sync::Arc, time::Duration};
use tokio::sync::watch;

#[derive(Clone)]
enum StartupState {
    Pending,
    Ready,
    Failed(Arc<Value>),
}

/// A concurrent native worker with startup gating, cancellation, and heartbeat support.
#[derive(Debug)]
pub struct Worker {
    transport: WorkerTransport,
    handler: Handler,
    hooks: PrestartHooks,
    progress: JobsProgress,
    control: WorkerControl,
}

impl Worker {
    /// Creates a worker from an owned handler and platform configuration.
    ///
    /// # Errors
    /// Returns invalid worker configuration or HTTP initialization errors.
    pub fn new(config: WorkerConfig, handler: Handler) -> Result<Self> {
        let progress = JobsProgress::new();
        let control = WorkerControl::new(config.concurrency, progress.clone());
        Ok(Self {
            transport: WorkerTransport::new(config)?,
            handler,
            hooks: PrestartHooks::new(),
            progress,
            control,
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
    /// Returns a shared shutdown, per-job cancellation, and concurrency observation handle.
    #[must_use]
    pub fn control(&self) -> WorkerControl {
        self.control.clone()
    }
    /// Start a production process with fitness checks, panic logging, and signal handling.
    /// Prestart failures are reported to held jobs before the process force-exits with code 1.
    /// The installed panic hook remains process-wide after successful shutdown.
    ///
    /// # Errors
    /// Returns signal-registration, concurrent-run, or startup-report transport errors.
    ///
    /// # Panics
    /// Installing the panic hook panics if the calling thread is already unwinding.
    pub async fn start(&self, fitness: &mut FitnessChecks) -> Result<()> {
        fitness.run_fitness_checks().await;
        std::panic::set_hook(Box::new(log_uncaught_panic));
        finish_process_start(self.run_until_signal().await)
    }
    /// Run until Ctrl-C or, on Unix, SIGTERM requests graceful shutdown.
    /// Signal registration is explicit; merely constructing a worker installs no handlers.
    ///
    /// # Errors
    /// Returns signal-registration errors or startup failures.
    pub async fn run_until_signal(&self) -> Result<()> {
        #[cfg(unix)]
        {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            self.run(async move {tokio::select! {
                _ = terminate.recv() => {},
                result = tokio::signal::ctrl_c() => {if let Err(error)=result {tracing::warn!(%error,"Ctrl-C listener failed");}}
            }}).await
        }
        #[cfg(not(unix))]
        self.run(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                tracing::warn!(%error,"Ctrl-C listener failed");
            }
        })
        .await
    }
    /// Finish fitness checks before heartbeats, intake, prestart hooks, or handlers begin.
    /// A fitness failure reports unhealthy and force-exits the entire process.
    ///
    /// # Errors
    /// Returns startup or worker errors after fitness checks have passed.
    pub async fn run_with_fitness(
        &self,
        fitness: &mut FitnessChecks,
        shutdown: impl Future<Output = ()>,
    ) -> Result<()> {
        fitness.run_fitness_checks().await;
        self.run(shutdown).await
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
            .await?;
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
    /// Run a local process job, force-exiting on prestart failure without installing a panic hook.
    /// Other local errors and successful output are returned to the caller.
    ///
    /// # Errors
    /// Returns invalid job input or handler stream errors; prestart failures terminate instead.
    pub async fn start_local(&self, job: Value) -> Result<Value> {
        finish_process_start(self.run_local(job).await)
    }
    /// Runs until shutdown resolves, then stops intake and drains active handlers.
    ///
    /// Startup hooks run concurrently with bounded intake. No handler runs before they
    /// finish. Stop-channel signals cancel individual handler futures. Dropping this
    /// future cancels all work and clears the job tracker.
    ///
    /// # Errors
    /// Returns startup failures, a failed fallback failure report, or an attempt to run concurrently.
    /// Individual job delivery failures are logged and do not stop sibling jobs.
    pub async fn run(&self, shutdown: impl Future<Output = ()>) -> Result<()> {
        let _guard = self.control.begin()?;
        let config = self.transport.config();
        let mut queue = VecDeque::<WorkerJob>::new();
        let mut running = FuturesUnordered::new();
        let mut intake = IntakeSchedule::new(config.concurrency);
        let mut take: Option<BoxFuture<'_, Result<Vec<WorkerJob>>>> = None;
        let mut stop = self.poll_stops(Duration::ZERO).boxed();
        let mut prestart = self
            .hooks
            .run_prestart_phase(config.prestart_timeout)
            .boxed();
        let mut prestart_done = !self.hooks.has_prestart_hooks();
        let mut took_request = false;
        let mut shutting_down = !self.control.is_alive();
        let (startup, _) = watch::channel(if prestart_done {
            StartupState::Ready
        } else {
            StartupState::Pending
        });
        let mut stopping = self.control.stopped();
        let shutdown = async move {
            tokio::select! { _ = shutdown => {}, _ = stopping.wait_for(|stop| *stop) => {} }
        }
        .fuse();
        let pings = self.ping_loop().fuse();
        futures_util::pin_mut!(shutdown, pings);
        loop {
            while running.len() < self.control.current_concurrency() {
                let Some(job) = queue.pop_front() else {
                    break;
                };
                let id = job.id.clone();
                let (abort, registration) = AbortHandle::new_pair();
                self.control.insert(id.clone(), abort);
                let gate = startup.subscribe();
                running.push(async move {
                    (
                        id,
                        Abortable::new(self.handle_after_prestart(job, gate), registration).await,
                    )
                });
            }
            if shutting_down && running.is_empty() && queue.is_empty() {
                return Ok(());
            }
            self.control.queued(queue.len());
            if !shutting_down
                && take.is_none()
                && let Some(free) =
                    intake.poll(self.control.current_occupancy(), config, &self.control)
            {
                take = Some(self.transport.get_job(free, &self.progress).boxed());
            }
            let deadline = intake.deadline();
            tokio::select! {
                _ = &mut shutdown, if !shutting_down => {
                    shutting_down=true;
                    self.control.kill_worker();
                    take=None;
                    if !prestart_done {
                        startup.send_replace(StartupState::Failed(Arc::new(self.startup_payload(true, "worker shutdown began before prestart completed"))));
                    }
                },
                result = &mut prestart, if !prestart_done && !shutting_down => {
                    prestart_done=true;
                    if let Err(error)=result {
                        // The failed worker stops both long polls before its bounded reporting phase.
                        drop(take.take());
                        drop(stop);
                        self.control.kill_worker();
                        let payload=Arc::new(self.startup_payload(false, &error.to_string()));
                        startup.send_replace(StartupState::Failed(payload.clone()));
                        let report=async {
                            while let Some((id,result))=running.next().await {
                                self.progress.remove(&id);self.control.remove(&id);
                                if let Ok(Err(error))=result {tracing::warn!(%error,"startup failure delivery failed");}
                            }
                            for job in queue.drain(..) {
                                if let Err(error)=self.fail_startup_job(&job,&payload).await {tracing::warn!(%error,"startup failure delivery failed");}
                                self.progress.remove(&job.id);
                            }
                            if !took_request {self.claim_one_job_to_fail(&payload).await?;}
                            Ok::<(),Error>(())
                        };
                        match tokio::time::timeout(config.prestart_failure_report_timeout,report).await {
                            Ok(result)=>result?,
                            Err(_)=>tracing::warn!("timed out delivering startup failure"),
                        }
                        return Err(Error::Prestart(error));
                    }
                    startup.send_replace(StartupState::Ready);
                },
                result = async {match take.as_mut() {Some(future)=>future.await,None=>std::future::pending().await}}, if take.is_some() => {
                    take=None;
                    let delay=match result {
                        Ok(jobs) if !jobs.is_empty()=> {
                            took_request=true;
                            for job in jobs {
                                self.progress.add(job.clone());queue.push_back(job);
                            }
                            Duration::ZERO
                        },
                        Ok(_)=>config.idle_interval,
                        Err(error)=> {
                            tracing::warn!(%error,"job intake failed");
                            if error.status_code()==Some(429) {Duration::from_secs(5)} else {config.idle_interval}
                        }
                    };
                    intake.after_response(delay);
                    tokio::task::yield_now().await;
                },
                Some((id,result)) = running.next(), if !running.is_empty() => {
                    self.progress.remove(&id); self.control.remove(&id);
                    match result {
                        Ok(Ok(())) if config.refresh_worker=>{shutting_down=true;self.control.kill_worker();take=None;},
                        Ok(Err(error))=>tracing::error!(%error,"job delivery failed"),
                        _=>{}
                    }
                },
                result = &mut stop, if !shutting_down => {
                    let mut delay=Duration::from_secs(1);
                    match result {
                        Ok(ids)=> {
                            if !ids.is_empty() {delay=Duration::ZERO;}
                            for id in ids {self.control.stop_job(&id);}
                        },
                        Err(error)=>{tracing::warn!(%error,"stop channel failed");if error.status_code()==Some(429) {delay=Duration::from_secs(5);} else if matches!(error,Error::Http(ref e) if e.is_timeout()) {delay=Duration::ZERO;}}
                    }
                    stop=self.poll_stops(delay).boxed();
                },
                _ = &mut pings => {},
                _ = async {if let Some(deadline)=deadline {tokio::time::sleep_until(deadline).await} else {std::future::pending().await}}, if !shutting_down && take.is_none() => {},
            }
        }
    }
    fn startup_payload(&self, cancelled: bool, message: &str) -> Value {
        json!({"event":if cancelled {"prestart_cancelled"} else {"prestart_failed"},
            "error_type":if cancelled {"PrestartCancelled"} else {"PrestartError"},
            "error_message":message, "worker_id":self.transport.config().worker_id,"runpod_version":env!("CARGO_PKG_VERSION")})
    }
    async fn fail_startup_job(&self, job: &WorkerJob, payload: &Value) -> Result<()> {
        self.transport
            .send_result(
                &json!({"error":payload.to_string()}),
                job,
                ResultKind::Single,
            )
            .await
    }
    async fn handle_after_prestart(
        &self,
        job: WorkerJob,
        mut gate: watch::Receiver<StartupState>,
    ) -> Result<()> {
        let state = gate
            .wait_for(|state| !matches!(state, StartupState::Pending))
            .await
            .map_err(|_| Error::InvalidInput("startup gate closed before completion".into()))?
            .clone();
        match state {
            StartupState::Ready => self.handle_job(job).await,
            StartupState::Failed(payload) => self.fail_startup_job(&job, &payload).await,
            StartupState::Pending => unreachable!("the startup wait predicate excludes pending"),
        }
    }
    async fn claim_one_job_to_fail(&self, payload: &Value) -> Result<()> {
        match tokio::time::timeout(
            self.transport.config().prestart_claim_timeout,
            self.transport.get_job(1, &self.progress),
        )
        .await
        {
            Ok(Ok(jobs)) => {
                for job in jobs {
                    self.fail_startup_job(&job, payload).await?;
                }
            }
            Ok(Err(error)) => {
                tracing::warn!(%error,"failed to claim a request for startup reporting")
            }
            Err(_) => tracing::warn!("startup reporting claim timed out"),
        }
        Ok(())
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
/// Production startup installs a process-wide panic logger. Prestart failures force-exit.
///
/// # Errors
/// Returns invalid configuration, local input, startup, or transport errors.
///
/// # Panics
/// Production startup cannot install its panic hook while the calling thread is unwinding.
pub async fn start(handler: Handler) -> Result<()> {
    start_with_fitness(handler, &mut FitnessChecks::from_env()).await
}

/// Start a worker using custom fitness checks and any retained early-check state.
/// The registry's owned environment supplies worker configuration. Local mode skips fitness.
/// A production fitness failure reports unhealthy and force-exits the entire process.
///
/// # Errors
/// Returns invalid configuration, local input, startup, or transport errors.
///
/// # Panics
/// Production startup cannot install its panic hook while the calling thread is unwinding.
pub async fn start_with_fitness(handler: Handler, fitness: &mut FitnessChecks) -> Result<()> {
    let config = WorkerConfig::from_values(fitness.environment())?;
    let local = config.job_get_url.is_none();
    let worker = Worker::new(config, handler)?;
    if local {
        let input = tokio::fs::read("test_input.json").await?;
        let result = worker.start_local(serde_json::from_slice(&input)?).await?;
        tracing::info!(output=%result,"local worker finished");
        if result.get("error").is_some_and(truthy) {
            return Err(Error::InvalidResponse(result["error"].to_string()));
        }
        Ok(())
    } else {
        worker.start(fitness).await
    }
}

fn finish_process_start<T>(result: Result<T>) -> Result<T> {
    match result {
        Err(Error::Prestart(error)) => {
            tracing::error!(%error, "prestart_failed | Worker is unhealthy, exiting.");
            super::health::terminate_unhealthy(1)
        }
        result => result,
    }
}

fn log_uncaught_panic(info: &std::panic::PanicHookInfo<'_>) {
    let backtrace = std::backtrace::Backtrace::force_capture();
    tracing::error!("Uncaught exception | {info}\n{backtrace}");
}
