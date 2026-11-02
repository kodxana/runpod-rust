use super::{
    CliError, CliResult,
    streams::{Destination, Streams},
};
use crate::{
    Client,
    apps::{
        deploy::{self, AppDefinition, ArtifactOptions, DeployOptions, DeployResult},
        discovery::{self, DiscoveredApp, WorkerBuildOptions},
        events::{self, EventSink},
        images::{DEFAULT_PYTHON_VERSION, SUPPORTED_PYTHON_VERSIONS},
    },
};
use clap::{Arg, ArgAction, ArgMatches, Command, parser::ValueSource};
use std::{
    collections::HashMap,
    fmt::Write,
    future::Future,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
#[path = "deployment_progress.rs"]
mod progress;
use progress::Progress;
#[cfg(test)]
use progress::bar;
#[cfg(test)]
#[path = "deployment_tests.rs"]
mod tests;

pub(super) fn command() -> Command {
    Command::new("deploy").about("Build and deploy discovered Cargo applications").args_override_self(true)
        .arg(Arg::new("target").value_parser(clap::value_parser!(PathBuf)).help("Cargo directory, manifest or binary source (default: current directory)"))
        .arg(Arg::new("env").long("env").short('e').allow_hyphen_values(true).help("Target environment"))
        .arg(Arg::new("build-only").long("build-only").action(ArgAction::SetTrue).help("Write <app>-artifact.tar.gz to the current directory without deploying"))
        .arg(Arg::new("runtime").long("runtime").value_parser(["native","python"]).default_value("native").help("Compiled Rust worker or Python-compatible source artifact"))
        .arg(Arg::new("worker").long("worker").value_parser(clap::value_parser!(PathBuf)).help("Already-compiled native worker; relative to the current directory"))
        .arg(Arg::new("target-triple").long("target-triple").default_value("x86_64-unknown-linux-gnu").help("Native worker Cargo target (toolchain and linker must be installed)"))
        .arg(Arg::new("profile").long("profile").default_value("release").help("Native worker Cargo profile"))
        .arg(Arg::new("python-version").long("python-version").help("Python artifact wheel/image version; requires --runtime python"))
        .arg(Arg::new("exclude").long("exclude").allow_hyphen_values(true).help("Comma-separated Python packages supplied by the worker image; requires --runtime python"))
}
enum Runtime {
    Native {
        worker: Option<PathBuf>,
        build: WorkerBuildOptions,
    },
    Python {
        options: ArtifactOptions,
        version: Option<String>,
    },
}
struct Options {
    target: PathBuf,
    environment: Option<String>,
    build_only: bool,
    runtime: Runtime,
}
impl Options {
    fn read(args: &ArgMatches) -> CliResult<Self> {
        let python = args
            .get_one::<String>("runtime")
            .is_some_and(|v| v == "python");
        let supplied = |key: &str| args.value_source(key) == Some(ValueSource::CommandLine);
        if python
            && ["worker", "target-triple", "profile"]
                .into_iter()
                .any(supplied)
        {
            return Err(CliError::Usage(
                "--worker, --target-triple and --profile require --runtime native".into(),
            ));
        }
        if !python && ["python-version", "exclude"].into_iter().any(supplied) {
            return Err(CliError::Usage(
                "--python-version and --exclude require --runtime python".into(),
            ));
        }
        if supplied("worker") && ["target-triple", "profile"].into_iter().any(supplied) {
            return Err(CliError::Usage(
                "--worker cannot be combined with --target-triple or --profile".into(),
            ));
        }
        let runtime = if python {
            let mut options = ArtifactOptions::default();
            options.build.exclude = args
                .get_one::<String>("exclude")
                .filter(|s| !s.is_empty())
                .map(|s| s.split(',').map(str::to_owned).collect())
                .unwrap_or_default();
            Runtime::Python {
                options,
                version: args.get_one::<String>("python-version").cloned(),
            }
        } else {
            Runtime::Native {
                worker: args.get_one::<PathBuf>("worker").cloned(),
                build: WorkerBuildOptions {
                    target: args
                        .get_one::<String>("target-triple")
                        .cloned()
                        .unwrap_or_default(),
                    profile: args
                        .get_one::<String>("profile")
                        .cloned()
                        .unwrap_or_default(),
                },
            }
        };
        Ok(Self {
            target: args
                .get_one::<PathBuf>("target")
                .cloned()
                .unwrap_or_else(|| ".".into()),
            environment: args.get_one::<String>("env").cloned(),
            build_only: args.get_flag("build-only"),
            runtime,
        })
    }
    async fn run(self, streams: &mut Streams<'_>) -> CliResult<()> {
        let cwd = std::env::current_dir().map_err(crate::Error::from)?;
        self.run_in(&cwd, streams).await
    }
    async fn run_in(mut self, cwd: &Path, streams: &mut Streams<'_>) -> CliResult<()> {
        let requested = cwd.join(&self.target);
        if !requested.try_exists().map_err(crate::Error::from)? {
            return Err(CliError::Failed(format!(
                "{} does not exist",
                requested.display()
            )));
        }
        let target = std::fs::canonicalize(&requested).map_err(crate::Error::from)?;
        if let Runtime::Python { options, version } = &mut self.runtime {
            let local = if version.is_some() {
                String::new()
            } else {
                local_python_version(&options.tools).await?
            };
            let (selected, warn) = select_python_version(version.as_deref(), &local);
            if warn {
                streams.write(Destination::Out,&format!("! local python has no runtime image; building for {DEFAULT_PYTHON_VERSION} (override with --python-version)\n"))?;
            }
            options.build.python_version = selected;
        }
        let apps = discovery::discover_apps(&target).await?;
        if apps.is_empty() {
            return Err(CliError::Failed("no runpod.App found. register an App and call discovery::export_if_requested before worker startup".into()));
        }
        if let Runtime::Native {
            worker: Some(worker),
            ..
        } = &mut self.runtime
        {
            if apps.iter().any(|app| app.executable != apps[0].executable) {
                return Err(CliError::Usage(
                    "--worker requires apps from a single Cargo binary; pass its source as TARGET"
                        .into(),
                ));
            }
            *worker = std::fs::canonicalize(cwd.join(&*worker)).map_err(crate::Error::from)?;
        }
        let client = if self.build_only {
            None
        } else {
            Some(Client::from_env()?)
        };
        let source_root = if target.is_dir() {
            target.as_path()
        } else {
            target.parent().unwrap_or(&target)
        };
        if !self.build_only && apps.len() > 1 {
            let rows = apps
                .iter()
                .map(|app| {
                    (
                        app.definition.name.as_str(),
                        source_name(app, source_root),
                        app.definition.resources.len(),
                    )
                })
                .collect::<Vec<_>>();
            streams.write(Destination::Out, &plan(&rows))?;
        }
        let mut workers = HashMap::new();
        for (index, app) in apps.iter().enumerate() {
            let environment = self
                .environment
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(&app.definition.environment);
            if !self.build_only {
                if index > 0 || apps.len() > 1 {
                    streams.write(Destination::Out, "\n")?;
                }
                streams.write(
                    Destination::Out,
                    &banner(&app.definition, environment, &source_name(app, source_root)),
                )?;
            }
            let action = client.as_ref().map_or_else(
                || Action::Build(cwd.join(format!("{}-artifact.tar.gz", app.definition.name))),
                Action::Deploy,
            );
            let started = Instant::now();
            let outcome = observe(streams, |events| {
                run_operation(
                    &self.runtime,
                    app,
                    action,
                    self.environment.as_deref(),
                    events,
                    &mut workers,
                )
            })
            .await?;
            match outcome {
                Outcome::Built(path, size) => streams.write(
                    Destination::Out,
                    &format!(
                        "\u{2713} built {} {} ({:.1} MB)\n",
                        app.definition.name,
                        path.file_name()
                            .unwrap_or(path.as_os_str())
                            .to_string_lossy(),
                        size as f64 / 1_048_576.0
                    ),
                )?,
                Outcome::Deployed(result) => {
                    streams.write(
                        Destination::Out,
                        &format!(
                            "\u{2713} {}/{environment} is live {:.1}s\n",
                            result.app_name,
                            started.elapsed().as_secs_f64()
                        ),
                    )?;
                    if !result.endpoints.is_empty() {
                        streams.write(Destination::Out, "\n")?;
                        let width = result
                            .endpoints
                            .keys()
                            .map(|name| name.chars().count())
                            .max()
                            .unwrap_or(0);
                        for (name, id) in result.endpoints {
                            let link = if streams.terminal {
                                format!(
                                    "\x1b]8;;https://console.runpod.io/serverless/user/endpoint/{id}?tab=overview\x1b\\{id} \u{2197}\x1b]8;;\x1b\\"
                                )
                            } else {
                                format!("{id} \u{2197}")
                            };
                            streams
                                .write(Destination::Out, &format!("  {name:<width$}  {link}\n"))?;
                        }
                    }
                }
            }
        }
        if !self.build_only {
            streams.write(Destination::Out, "\n")?;
        }
        Ok(())
    }
}
enum Action<'a> {
    Build(PathBuf),
    Deploy(&'a Client),
}
enum Outcome {
    Built(PathBuf, u64),
    Deployed(DeployResult),
}
async fn run_operation(
    runtime: &Runtime,
    app: &DiscoveredApp,
    action: Action<'_>,
    environment: Option<&str>,
    events: EventSink,
    workers: &mut HashMap<PathBuf, PathBuf>,
) -> crate::Result<Outcome> {
    let native = if let Runtime::Native { worker, build } = runtime {
        Some(if let Some(worker) = worker {
            worker.clone()
        } else if let Some(worker) = workers.get(&app.executable) {
            worker.clone()
        } else {
            events::emit(
                Some(&events),
                "phase",
                &[
                    crate::json!("compile"),
                    crate::json!(format!("{} ({})", build.target, build.profile)),
                ],
            );
            let worker = app.build_worker(build).await?;
            workers.insert(app.executable.clone(), worker.clone());
            worker
        })
    } else {
        None
    };
    match action {
        Action::Build(output) => {
            let definition = app.definition.clone();
            let root = app.project_root.clone();
            let python = match runtime {
                Runtime::Python { options, .. } => Some(options.clone()),
                Runtime::Native { .. } => None,
            };
            let artifact = tokio::task::spawn_blocking(move || {
                if let Some(worker) = native {
                    deploy::build_native_artifact(
                        &definition,
                        &root,
                        &worker,
                        Some(&output),
                        Some(&events),
                    )
                } else if let Some(options) = python {
                    deploy::build_artifact(
                        &definition,
                        &root,
                        &options,
                        Some(&output),
                        Some(&events),
                    )
                } else {
                    unreachable!("each runtime selects one artifact builder")
                }
            })
            .await??;
            Ok(Outcome::Built(
                artifact.path().to_owned(),
                std::fs::metadata(artifact.path())?.len(),
            ))
        }
        Action::Deploy(client) => {
            let result = if let Some(worker) = native {
                deploy::deploy_native_app(
                    client,
                    &app.definition,
                    &app.project_root,
                    &worker,
                    environment,
                    Some(&events),
                )
                .await?
            } else if let Runtime::Python { options, .. } = runtime {
                deploy::deploy_app(
                    client,
                    &app.definition,
                    &app.project_root,
                    &DeployOptions {
                        environment: environment.map(str::to_owned),
                        artifact: options.clone(),
                    },
                    Some(&events),
                )
                .await?
            } else {
                unreachable!("native runtime produces a worker path")
            };
            Ok(Outcome::Deployed(result))
        }
    }
}
fn select_python_version(explicit: Option<&str>, local: &str) -> (String, bool) {
    if let Some(version) = explicit {
        return (version.into(), false);
    }
    if SUPPORTED_PYTHON_VERSIONS.contains(&local) {
        (local.into(), false)
    } else {
        (DEFAULT_PYTHON_VERSION.into(), true)
    }
}
async fn local_python_version(tools: &crate::apps::build::BuildTools) -> crate::Result<String> {
    let mut command = tokio::process::Command::new(&tools.python);
    command
        .args(&tools.arguments)
        .args([
            "-c",
            "import sys; print(str(sys.version_info.major) + '.' + str(sys.version_info.minor))",
        ])
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = command.output().await?;
    if !output.status.success() {
        return Err(crate::Error::InvalidInput(format!(
            "cannot determine build interpreter version: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
fn source_name(app: &DiscoveredApp, root: &Path) -> String {
    app.source
        .strip_prefix(root)
        .unwrap_or(&app.source)
        .to_string_lossy()
        .into_owned()
}
fn plan(rows: &[(&str, String, usize)]) -> String {
    let mut output = format!("deploy {} apps\n", rows.len());
    let width = rows
        .iter()
        .map(|(name, _, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    for (name, source, count) in rows {
        let plural = if *count == 1 { "" } else { "s" };
        let detail = if source.is_empty() {
            String::new()
        } else {
            format!("{source}, ")
        };
        writeln!(output, "  {name:<width$}  {detail}{count} resource{plural}")
            .expect("write to String");
    }
    output
}

fn banner(app: &AppDefinition, environment: &str, source: &str) -> String {
    let names = app
        .resources
        .iter()
        .map(|r| r.spec.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let names = if names.is_empty() {
        "(no resources)"
    } else {
        &names
    };
    let origin = if source.is_empty() {
        String::new()
    } else {
        format!(" from {source}")
    };
    format!(
        "deploy {} \u{2192} {environment}{origin}, {names}\n",
        app.name
    )
}
async fn observe<T, F: Future<Output = crate::Result<T>>>(
    streams: &mut Streams<'_>,
    operation: impl FnOnce(EventSink) -> F,
) -> CliResult<T> {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let events = EventSink::new(move |name, args| {
        let _ = sender.send((name.to_owned(), args.to_vec()));
        Ok(())
    });
    let operation = operation(events);
    tokio::pin!(operation);
    let mut progress = Progress::default();
    let mut tick = tokio::time::interval(Duration::from_millis(63));
    let mut frame = 0;
    let result = loop {
        tokio::select! {
            result=&mut operation=>break result,
            Some((event,args))=receiver.recv()=>progress.event(&event,&args),
            _=tick.tick(),if streams.terminal=>{streams.frame(&progress.render(frame))?;frame=(frame+1)%8;},
        }
    };
    while let Ok((event, args)) = receiver.try_recv() {
        progress.event(&event, &args);
    }
    progress.close();
    let output = progress.render(frame);
    if streams.terminal {
        streams.frame(&output)?;
        streams.finish_frame();
    } else {
        streams.write(Destination::Out, &output)?;
    }
    Ok(result?)
}
pub(super) async fn execute(args: &ArgMatches, streams: &mut Streams<'_>) -> CliResult<()> {
    let options = Options::read(args)?;
    let result = tokio::select! {result=options.run(streams)=>result,signal=tokio::signal::ctrl_c()=>{signal.map_err(crate::Error::from)?;Err(CliError::Aborted)}};
    match result {
        Err(CliError::Api(crate::Error::Discovery(discovery::DiscoveryError::Interrupted))) => {
            Err(CliError::Aborted)
        }
        other => other,
    }
}
