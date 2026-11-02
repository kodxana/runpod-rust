use super::{CliError, CliResult, interaction::Interaction};
use crate::{Value, apps::AppsApiClient, client::required_str};
use clap::{Arg, ArgAction, ArgMatches, Command};
use std::fmt::Write;

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Registry,
    Secret,
}

impl Kind {
    fn noun(self) -> &'static str {
        match self {
            Self::Registry => "registry credential",
            Self::Secret => "secret",
        }
    }

    pub(super) fn command(self) -> Command {
        let name = match self {
            Self::Registry => "registry",
            Self::Secret => "secret",
        };
        let add = Command::new("add")
            .args_override_self(true)
            .arg(Arg::new("name"));
        let add = match self {
            Self::Registry => add
                .arg(
                    Arg::new("username")
                        .long("username")
                        .allow_hyphen_values(true),
                )
                .arg(
                    Arg::new("password")
                        .long("password")
                        .allow_hyphen_values(true),
                ),
            Self::Secret => add
                .arg(Arg::new("value").long("value").allow_hyphen_values(true))
                .arg(
                    Arg::new("description")
                        .long("description")
                        .allow_hyphen_values(true)
                        .default_value(""),
                ),
        };
        Command::new(name)
            .about(format!("Manage {}s", self.noun()))
            .subcommand_required(true)
            .arg_required_else_help(true)
            .subcommand(add.about("Create a credential; prompt for omitted fields"))
            .subcommand(Command::new("list").about("List names without exposing values"))
            .subcommand(
                Command::new("delete")
                    .about("Delete by name")
                    .arg(Arg::new("name").required(true))
                    .arg(
                        Arg::new("yes")
                            .long("yes")
                            .short('y')
                            .action(ArgAction::SetTrue),
                    ),
            )
    }

    async fn list(self, api: &AppsApiClient) -> crate::Result<Vec<Value>> {
        match self {
            Self::Registry => api.list_registry_auths().await,
            Self::Secret => api.list_secrets().await,
        }
    }
}

// No derived Debug: add operations contain passwords or secret values.
pub(super) enum Operation {
    RegistryAdd {
        name: String,
        username: String,
        password: String,
    },
    SecretAdd {
        name: String,
        value: String,
        description: String,
    },
    List(Kind),
    Delete {
        kind: Kind,
        name: String,
    },
}

impl Operation {
    // Resolve prompts before credentials are loaded, as in the source CLI.
    pub(super) fn read(
        kind: Kind,
        args: &ArgMatches,
        ui: &mut impl Interaction,
    ) -> CliResult<Self> {
        match args.subcommand() {
            Some(("add", args)) => {
                let name = text(args, "name", "pass a NAME", ui)?;
                Ok(match kind {
                    Kind::Registry => Self::RegistryAdd {
                        name,
                        username: text(args, "username", "pass --username", ui)?,
                        password: hidden(args, "password", ui)?,
                    },
                    Kind::Secret => Self::SecretAdd {
                        name,
                        value: hidden(args, "value", ui)?,
                        description: args
                            .get_one::<String>("description")
                            .cloned()
                            .unwrap_or_default(),
                    },
                })
            }
            Some(("list", _)) => Ok(Self::List(kind)),
            Some(("delete", args)) => {
                let name = args
                    .get_one::<String>("name")
                    .ok_or_else(|| CliError::Usage("pass a NAME".into()))?;
                ui.confirm(
                    &format!("delete {} '{name}'?", kind.noun()),
                    args.get_flag("yes"),
                )?;
                Ok(Self::Delete {
                    kind,
                    name: name.clone(),
                })
            }
            _ => Err(CliError::Usage("a command is required".into())),
        }
    }

    pub(super) async fn execute(self, api: &AppsApiClient) -> CliResult<String> {
        match self {
            Self::RegistryAdd {
                name,
                username,
                password,
            } => {
                api.create_registry_auth(&name, &username, &password)
                    .await?;
                Ok(format!(
                    "✓ registry credential {name} created, use registry_auth=\"{name}\"\n"
                ))
            }
            Self::SecretAdd {
                name,
                value,
                description,
            } => {
                api.create_secret(&name, &value, &description).await?;
                Ok(format!(
                    "✓ secret {name} created, use runpod.Secret(\"{name}\")\n"
                ))
            }
            Self::List(kind) => show(kind, &kind.list(api).await?),
            Self::Delete { kind, name } => {
                let records = kind.list(api).await?;
                // Lookup is exact and takes the first match, including duplicate names.
                let mut found = None;
                for record in &records {
                    if required_str(record, "name")? == name {
                        found = Some(required_str(record, "id")?);
                        break;
                    }
                }
                let id = found.ok_or_else(|| {
                    CliError::Failed(format!("no {} named '{name}'", kind.noun()))
                })?;
                match kind {
                    Kind::Registry => {
                        api.delete_registry_auth(id).await?;
                    }
                    Kind::Secret => {
                        api.delete_secret(id).await?;
                    }
                }
                // Python reports success even when secretDelete returns false.
                Ok(format!("✓ deleted {} {name}\n", kind.noun()))
            }
        }
    }
}

fn text(
    args: &ArgMatches,
    name: &str,
    missing: &str,
    ui: &mut impl Interaction,
) -> CliResult<String> {
    if let Some(value) = args.get_one::<String>(name) {
        return Ok(value.clone());
    }
    if !ui.interactive() {
        return Err(CliError::Usage(format!(
            "{missing} in a non-interactive shell"
        )));
    }
    ui.text_prompt(name)
}

fn hidden(args: &ArgMatches, name: &str, ui: &mut impl Interaction) -> CliResult<String> {
    if let Some(value) = args.get_one::<String>(name) {
        return Ok(value.clone());
    }
    if !ui.interactive() {
        return Err(CliError::Usage(format!(
            "pass --{name} in a non-interactive shell"
        )));
    }
    ui.hidden_prompt(name)
}

fn show(kind: Kind, records: &[Value]) -> CliResult<String> {
    if records.is_empty() {
        return Ok(match kind {
            Kind::Registry => "\n  no registry credentials. run rp registry add <name>\n\n",
            Kind::Secret => "\n  no secrets. run rp secret add <name>\n\n",
        }
        .into());
    }
    let mut rows = records
        .iter()
        .map(|r| {
            let name = required_str(r, "name")?;
            let detail = match kind {
                Kind::Registry => required_str(r, "id")?,
                Kind::Secret => r["description"].as_str().unwrap_or_default(),
            };
            Ok((name, detail))
        })
        .collect::<crate::Result<Vec<_>>>()?;
    rows.sort_by(|a, b| a.0.cmp(b.0));
    let width = match kind {
        Kind::Registry => 0,
        Kind::Secret => rows
            .iter()
            .map(|(name, _)| name.chars().count())
            .max()
            .unwrap_or(0),
    };
    let mut output = String::from("\n");
    for (name, detail) in rows {
        writeln!(output, "  {name:<width$}  {detail}").expect("writing to a string cannot fail");
    }
    output.push('\n');
    Ok(output)
}
