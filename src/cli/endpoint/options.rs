use super::super::{CliError, CliResult};
use crate::configuration::{
    ContainerOptions, EndpointOptions, EndpointType, Flashboot, ScaleOptions,
};
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};

fn flag(name: &'static str) -> Arg {
    Arg::new(name).long(name).action(ArgAction::SetTrue)
}
fn text(name: &'static str) -> Arg {
    Arg::new(name).long(name).allow_hyphen_values(true)
}
fn multiple(name: &'static str) -> Arg {
    text(name).action(ArgAction::Append)
}
fn uint(name: &'static str, minimum: u64) -> Arg {
    text(name).value_parser(value_parser!(u64).range(minimum..))
}
fn finite(text: &str) -> Result<f64, String> {
    text.parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
        .ok_or_else(|| "expected a finite number".into())
}
fn delay(text: &str) -> Result<f64, String> {
    finite(text).and_then(|n| {
        if n >= 0.5 {
            Ok(n)
        } else {
            Err("queue delay must be at least 0.5 seconds".into())
        }
    })
}
fn interval(text: &str) -> Result<f64, String> {
    finite(text).and_then(|n| {
        if n >= 1.0 {
            Ok(n)
        } else {
            Err("interval must be at least 1 second".into())
        }
    })
}
fn scale(command: Command) -> Command {
    command
        .arg(uint("min-workers", 0))
        .arg(uint("max-workers", 0))
        .arg(text("idle-timeout-seconds").value_parser(value_parser!(u64).range(1..=3600)))
        .arg(text("queue-delay-seconds").value_parser(delay))
        .arg(uint("requests-per-worker", 1))
}
fn config(command: Command, creating: bool) -> Command {
    let mut command = command.arg(text("name").required(creating));
    if creating {
        command = command.arg(
            text("type")
                .value_parser(["queue", "load-balancer"])
                .ignore_case(true)
                .required(true),
        );
    }
    command = scale(command)
        .arg(uint("gpu-count", 1))
        .arg(uint("vcpu-count", 2))
        .arg(uint("request-timeout-seconds", 0))
        .arg(uint("disk", 1))
        .arg(
            text("flashboot")
                .value_parser(["off", "standard", "priority"])
                .ignore_case(true),
        );
    for name in ["image", "template", "args", "entrypoint", "cmd", "registry"] {
        command = command.arg(text(name));
    }
    for name in [
        "gpu",
        "cuda",
        "cpu",
        "data-center",
        "region",
        "network-volume",
        "global-volume",
        "env",
        "port",
    ] {
        command = command.arg(multiple(name));
    }
    if !creating {
        command = command.arg(multiple("unset-env"));
    }
    command
}
fn base(name: &'static str, about: &'static str) -> Command {
    Command::new(name).about(about).args_override_self(true)
}
fn id(command: Command, required: bool) -> Command {
    command.arg(
        Arg::new("endpoint-id")
            .value_name("ENDPOINT_ID")
            .required(required),
    )
}
fn timeout(command: Command) -> Command {
    command.arg(text("timeout").value_parser(finite).default_value("600"))
}
fn job_command() -> Command {
    let mut command = base("job", "Inspect and control queue jobs")
        .subcommand_required(true)
        .arg_required_else_help(true);
    for (name, about) in [
        ("get", "Show a job's status and output"),
        ("cancel", "Cancel a queued or running job"),
        ("retry", "Requeue a failed or timed-out job"),
    ] {
        command = command.subcommand(
            id(base(name, about), true)
                .arg(Arg::new("job-id").value_name("JOB_ID").required(true))
                .arg(flag("json")),
        );
    }
    let wait = id(base("wait", "Wait for a job and print its output"), true)
        .arg(Arg::new("job-id").value_name("JOB_ID").required(true))
        .arg(text("timeout").value_parser(|value: &str| {
            finite(value).and_then(|seconds| {
                if seconds > 0.0 {
                    Ok(seconds)
                } else {
                    Err("timeout must be positive".into())
                }
            })
        }))
        .arg(flag("json"));
    command = command.subcommand(wait).subcommand(
        id(
            base("stream", "Print streamed job output as it arrives"),
            true,
        )
        .arg(Arg::new("job-id").value_name("JOB_ID").required(true))
        .arg(flag("json")),
    );
    command
}
pub(in crate::cli) fn command() -> Command {
    base(
        "endpoint",
        "Create, inspect, scale and observe serverless endpoints",
    )
    .subcommand_required(true)
    .arg_required_else_help(true)
    .subcommand(job_command())
    .subcommand(
        id(
            base("purge", "Remove queued jobs, leaving running jobs intact"),
            true,
        )
        .arg(flag("yes").short('y'))
        .arg(flag("json")),
    )
    .subcommand(
        base("list", "List endpoints")
            .arg(text("name"))
            .arg(
                text("type")
                    .value_parser(["queue", "load-balancer"])
                    .ignore_case(true),
            )
            .arg(flag("json")),
    )
    .subcommand(id(base("get", "Show endpoint configuration"), false).arg(flag("json")))
    .subcommand(config(base("create", "Create an endpoint"), true).arg(flag("json")))
    .subcommand(
        timeout(config(
            id(base("update", "Change endpoint configuration"), true),
            false,
        ))
        .arg(flag("wait"))
        .arg(flag("json")),
    )
    .subcommand(
        timeout(id(
            base("edit", "Edit configuration in $EDITOR or apply a JSON file"),
            false,
        ))
        .arg(text("file"))
        .arg(flag("yes").short('y'))
        .arg(flag("no-follow"))
        .arg(flag("json")),
    )
    .subcommand(
        scale(id(
            base("scale", "Change worker limits and autoscaling"),
            true,
        ))
        .arg(flag("json")),
    )
    .subcommand(
        id(base("delete", "Delete an endpoint and its workers"), true).arg(flag("yes").short('y')),
    )
    .subcommand(
        id(base("workers", "Show endpoint workers"), true)
            .arg(flag("watch").short('w'))
            .arg(text("interval").value_parser(interval).default_value("2"))
            .arg(flag("json")),
    )
    .subcommand(
        id(
            base("releases", "Show release history and rollout progress"),
            true,
        )
        .arg(flag("json")),
    )
}
fn string(args: &ArgMatches, name: &str) -> Option<String> {
    args.try_get_one::<String>(name).ok().flatten().cloned()
}
fn strings(args: &ArgMatches, name: &str) -> Vec<String> {
    args.try_get_many::<String>(name)
        .ok()
        .flatten()
        .into_iter()
        .flatten()
        .cloned()
        .collect()
}
fn number<T: Clone + Send + Sync + 'static>(args: &ArgMatches, name: &str) -> Option<T> {
    args.try_get_one::<T>(name).ok().flatten().cloned()
}
fn enabled(args: &ArgMatches, name: &str) -> bool {
    number(args, name).unwrap_or(false)
}
fn scaling(args: &ArgMatches) -> ScaleOptions {
    ScaleOptions {
        min_workers: number(args, "min-workers"),
        max_workers: number(args, "max-workers"),
        idle_timeout_seconds: number(args, "idle-timeout-seconds"),
        queue_delay_seconds: number(args, "queue-delay-seconds"),
        requests_per_worker: number(args, "requests-per-worker"),
    }
}
pub(in crate::cli) struct Options {
    pub action: String,
    pub id: Option<String>,
    pub json: bool,
    pub config: EndpointOptions,
    pub kind: Option<EndpointType>,
    pub wait: bool,
    pub timeout: f64,
    pub yes: bool,
    pub no_follow: bool,
    pub(super) document: Option<Document>,
    pub watch: bool,
    pub interval: f64,
}
pub(super) enum Document {
    Stdin,
    File(std::fs::File),
}
impl Options {
    pub fn read(args: &ArgMatches) -> CliResult<Self> {
        let (action, args) = args
            .subcommand()
            .ok_or_else(|| CliError::Usage("an endpoint command is required".into()))?;
        let options = Self {
            action: action.into(),
            id: string(args, "endpoint-id"),
            json: enabled(args, "json"),
            config: EndpointOptions {
                container: ContainerOptions {
                    image: string(args, "image"),
                    template: string(args, "template"),
                    args: string(args, "args"),
                    disk: number(args, "disk"),
                    cmd: string(args, "cmd"),
                    entrypoint: string(args, "entrypoint"),
                    registry: string(args, "registry"),
                    env: strings(args, "env"),
                    ports: strings(args, "port"),
                    ..ContainerOptions::default()
                },
                name: string(args, "name"),
                flashboot: string(args, "flashboot")
                    .map(|v| v.parse::<Flashboot>())
                    .transpose()?,
                request_timeout_seconds: number(args, "request-timeout-seconds"),
                gpu: strings(args, "gpu"),
                gpu_count: number(args, "gpu-count"),
                cuda: strings(args, "cuda"),
                cpu: strings(args, "cpu"),
                vcpu_count: number(args, "vcpu-count"),
                scale: scaling(args),
                data_centers: strings(args, "data-center"),
                regions: strings(args, "region"),
                network_volumes: strings(args, "network-volume"),
                global_volumes: strings(args, "global-volume"),
                unset_env: strings(args, "unset-env"),
            },
            kind: string(args, "type")
                .map(|v| v.to_lowercase().parse::<EndpointType>())
                .transpose()?,
            wait: enabled(args, "wait"),
            timeout: number(args, "timeout").unwrap_or(600.0),
            yes: enabled(args, "yes"),
            no_follow: enabled(args, "no-follow"),
            document: string(args, "file")
                .map(|path| {
                    if path == "-" {
                        Ok(Document::Stdin)
                    } else {
                        std::fs::File::open(&path)
                            .map(Document::File)
                            .map_err(|error| {
                                CliError::Usage(format!(
                                    "Invalid value for --file: '{path}': {error}"
                                ))
                            })
                    }
                })
                .transpose()?,
            watch: enabled(args, "watch"),
            interval: number(args, "interval").unwrap_or(2.0),
        };
        if action == "create"
            && options
                .config
                .container
                .image
                .as_deref()
                .unwrap_or_default()
                .is_empty()
            && options
                .config
                .container
                .template
                .as_deref()
                .unwrap_or_default()
                .is_empty()
        {
            return Err(CliError::Usage("pass --image or --template".into()));
        }
        Ok(options)
    }
}
