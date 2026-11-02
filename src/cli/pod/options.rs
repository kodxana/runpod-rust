use super::{CliError, CliResult, config::Container};
use crate::configuration::ContainerOptions;
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};

fn flag(name: &'static str) -> Arg {
    Arg::new(name).long(name).action(ArgAction::SetTrue)
}
fn text(name: &'static str) -> Arg {
    Arg::new(name).long(name).allow_hyphen_values(true)
}
fn many(name: &'static str) -> Arg {
    text(name).action(ArgAction::Append)
}
fn number(name: &'static str, min: u64) -> Arg {
    text(name).value_parser(value_parser!(u64).range(min..))
}
fn base(name: &'static str, about: &'static str) -> Command {
    Command::new(name).about(about).args_override_self(true)
}
fn pod_id(command: Command) -> Command {
    command.arg(Arg::new("pod-id").value_name("POD_ID"))
}
fn container(mut command: Command) -> Command {
    for name in [
        "image",
        "template",
        "args",
        "cmd",
        "entrypoint",
        "registry",
        "persistent-volume",
        "network-volume",
        "global-volume",
    ] {
        command = command.arg(text(name));
    }
    command
        .arg(many("env").short('e'))
        .arg(many("port").short('p'))
        .arg(number("disk", 1))
        .arg(flag("global-networking").overrides_with("no-global-networking"))
        .arg(flag("no-global-networking").overrides_with("global-networking"))
}
pub(in crate::cli) fn command() -> Command {
    let mut command = base("pod", "Create and manage pods")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            base("list", "List pods")
                .arg(text("name"))
                .arg(
                    many("status")
                        .value_parser([
                            "PROVISIONING",
                            "STARTING",
                            "RUNNING",
                            "EXITED",
                            "ERROR",
                            "TERMINATED",
                        ])
                        .ignore_case(true),
                )
                .arg(flag("include-cluster-pods"))
                .arg(flag("json")),
        )
        .subcommand(pod_id(base("get", "Show a pod and its connection details")).arg(flag("json")))
        .subcommand(
            container(base("create", "Create a GPU or CPU pod"))
                .arg(Arg::new("name").value_name("NAME"))
                .arg(text("gpu"))
                .arg(number("gpu-count", 1))
                .arg(text("cpu"))
                .arg(number("vcpu", 2))
                .arg(
                    text("cloud")
                        .value_parser(["secure", "community"])
                        .ignore_case(true),
                )
                .arg(many("region"))
                .arg(many("data-center"))
                .arg(many("cuda"))
                .arg(number("min-ram-per-gpu", 1))
                .arg(number("min-vcpu-per-gpu", 1))
                .arg(flag("ssh"))
                .arg(flag("jupyter"))
                .arg(flag("wait"))
                .arg(flag("json")),
        )
        .subcommand(
            container(pod_id(base("update", "Change pod settings")))
                .arg(text("name"))
                .arg(many("unset-env"))
                .arg(flag("lock").overrides_with("unlock"))
                .arg(flag("unlock").overrides_with("lock"))
                .arg(flag("json")),
        )
        .subcommand(
            pod_id(base("terminate", "Permanently terminate a pod"))
                .visible_alias("delete")
                .arg(flag("yes").short('y')),
        )
        .subcommand(
            super::super::logs::command("pod-id").mut_arg("pod-id", |arg| arg.required(false)),
        );
    for name in ["start", "stop", "restart"] {
        let mut action = pod_id(base(name, "Change pod running state")).arg(flag("json"));
        if name != "stop" {
            action = action.arg(flag("wait"));
        }
        command = command.subcommand(action);
    }
    command
}
fn string(args: &ArgMatches, key: &str) -> Option<String> {
    args.get_one::<String>(key).cloned()
}
fn strings(args: &ArgMatches, key: &str) -> Vec<String> {
    args.get_many::<String>(key)
        .map(|v| v.cloned().collect())
        .unwrap_or_default()
}
fn uint(args: &ArgMatches, key: &str) -> Option<u64> {
    args.get_one::<u64>(key).copied()
}
fn toggle(args: &ArgMatches, on: &str, off: &str) -> Option<bool> {
    if args.get_flag(on) {
        Some(true)
    } else if args.get_flag(off) {
        Some(false)
    } else {
        None
    }
}
fn read_container(args: &ArgMatches) -> Container {
    Container {
        options: ContainerOptions {
            image: string(args, "image"),
            template: string(args, "template"),
            args: string(args, "args"),
            disk: uint(args, "disk"),
            global_networking: toggle(args, "global-networking", "no-global-networking"),
            cmd: string(args, "cmd"),
            entrypoint: string(args, "entrypoint"),
            ports: strings(args, "port"),
            registry: string(args, "registry"),
            env: strings(args, "env"),
        },
        persistent: string(args, "persistent-volume"),
        network: string(args, "network-volume"),
        global: string(args, "global-volume"),
    }
}
pub(super) enum Hardware {
    Gpu {
        spec: String,
        count: Option<u64>,
        cuda: Vec<String>,
        min_ram: Option<u64>,
        min_vcpu: Option<u64>,
    },
    Cpu {
        spec: String,
        vcpu: Option<u64>,
    },
}
pub(super) struct Create {
    pub name: Option<String>,
    pub hardware: Hardware,
    pub cloud: Option<String>,
    pub regions: Vec<String>,
    pub datacenters: Vec<String>,
    pub container: Container,
    pub ssh: bool,
    pub jupyter: bool,
    pub wait: bool,
}
pub(super) struct Update {
    pub id: Option<String>,
    pub name: Option<String>,
    pub container: Container,
    pub unset_env: Vec<String>,
    pub locked: Option<bool>,
}
#[derive(Clone, Copy)]
pub(super) enum Action {
    Start,
    Stop,
    Restart,
    Terminate,
}
impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Terminate => "terminate",
        }
    }
    pub fn done(self) -> &'static str {
        match self {
            Self::Start => "started",
            Self::Stop => "stopped",
            Self::Restart => "restarted",
            Self::Terminate => "terminated",
        }
    }
}
pub(super) enum Operation {
    List {
        name: Option<String>,
        statuses: Vec<String>,
        include_cluster: bool,
    },
    Get(Option<String>),
    Create(Box<Create>),
    Update(Box<Update>),
    Action {
        id: Option<String>,
        action: Action,
        wait: bool,
    },
    Terminate {
        id: Option<String>,
        yes: bool,
    },
}
pub(super) struct Options {
    pub operation: Operation,
    pub as_json: bool,
}
impl Options {
    pub fn read(args: &ArgMatches) -> CliResult<Self> {
        let (name, args) = args
            .subcommand()
            .ok_or_else(|| CliError::Usage("a pod command is required".into()))?;
        let operation = match name {
            "list" => Operation::List {
                name: string(args, "name"),
                statuses: strings(args, "status")
                    .into_iter()
                    .map(|s| s.to_lowercase())
                    .collect(),
                include_cluster: args.get_flag("include-cluster-pods"),
            },
            "get" => Operation::Get(string(args, "pod-id")),
            "create" => {
                let gpu = string(args, "gpu").filter(|s| !s.is_empty());
                let cpu = string(args, "cpu").filter(|s| !s.is_empty());
                let count = uint(args, "gpu-count");
                let vcpu = uint(args, "vcpu");
                let cuda = strings(args, "cuda");
                let min_ram = uint(args, "min-ram-per-gpu");
                let min_vcpu = uint(args, "min-vcpu-per-gpu");
                let hardware = match (gpu, cpu) {
                    (Some(_), Some(_)) => {
                        return Err(CliError::Usage("use either --gpu or --cpu".into()));
                    }
                    (None, None) => {
                        return Err(CliError::Usage(
                            "pass --gpu (like 4090 or h100:2) or --cpu (like cpu3c)".into(),
                        ));
                    }
                    (None, Some(spec)) => {
                        if count.is_some()
                            || !cuda.is_empty()
                            || min_ram.is_some()
                            || min_vcpu.is_some()
                        {
                            return Err(CliError::Usage("--gpu-count, --cuda, --min-ram-per-gpu, and --min-vcpu-per-gpu apply to gpu pods".into()));
                        }
                        Hardware::Cpu { spec, vcpu }
                    }
                    (Some(spec), None) => {
                        if vcpu.is_some() {
                            return Err(CliError::Usage("--vcpu applies to cpu pods".into()));
                        }
                        Hardware::Gpu {
                            spec,
                            count,
                            cuda,
                            min_ram,
                            min_vcpu,
                        }
                    }
                };
                let container = read_container(args);
                if container
                    .options
                    .image
                    .as_ref()
                    .is_none_or(String::is_empty)
                    && container
                        .options
                        .template
                        .as_ref()
                        .is_none_or(String::is_empty)
                {
                    return Err(CliError::Usage("pass --image or --template".into()));
                }
                Operation::Create(Box::new(Create {
                    name: string(args, "name").filter(|s| !s.is_empty()),
                    hardware,
                    cloud: string(args, "cloud"),
                    regions: strings(args, "region"),
                    datacenters: strings(args, "data-center"),
                    container,
                    ssh: args.get_flag("ssh"),
                    jupyter: args.get_flag("jupyter"),
                    wait: args.get_flag("wait"),
                }))
            }
            "update" => {
                let update = Update {
                    id: string(args, "pod-id"),
                    name: string(args, "name"),
                    container: read_container(args),
                    unset_env: strings(args, "unset-env"),
                    locked: toggle(args, "lock", "unlock"),
                };
                if update.name.is_none()
                    && update.locked.is_none()
                    && update.unset_env.is_empty()
                    && update.container == Container::default()
                {
                    return Err(CliError::Usage(
                        "nothing to update. see rp pod update --help".into(),
                    ));
                }
                Operation::Update(Box::new(update))
            }
            "start" | "stop" | "restart" => Operation::Action {
                id: string(args, "pod-id"),
                action: match name {
                    "start" => Action::Start,
                    "stop" => Action::Stop,
                    _ => Action::Restart,
                },
                wait: name != "stop" && args.get_flag("wait"),
            },
            "terminate" => Operation::Terminate {
                id: string(args, "pod-id"),
                yes: args.get_flag("yes"),
            },
            _ => return Err(CliError::Usage("unknown pod command".into())),
        };
        Ok(Self {
            operation,
            as_json: name != "terminate" && args.get_flag("json"),
        })
    }
}
