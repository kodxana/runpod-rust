use super::{CliError, CliResult, api, interaction::Interaction, output};
use crate::{Client, Error, Result, Value, compute, json};
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};

pub(super) fn command() -> Command {
    let json = || {
        Arg::new("json")
            .long("json")
            .action(ArgAction::SetTrue)
            .help("Print JSON for scripts and agents")
    };
    let global = || {
        Arg::new("global")
            .long("global")
            .action(ArgAction::SetTrue)
            .help("Manage global volumes, which mount in any data center")
    };
    let id = || Arg::new("id").value_name("VOLUME_ID");
    let size = || {
        Arg::new("size")
            .long("size")
            .value_parser(value_parser!(u32).range(10..=4096))
            .help("Storage in GB (10-4096); network volumes can only grow")
    };
    Command::new("volume")
        .about("Create, list, resize, and delete network and global volumes")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("list")
                .about("List volumes with their size, data center, and type")
                .args_override_self(true)
                .arg(global())
                .arg(json()),
        )
        .subcommand(
            Command::new("get")
                .about("Show one volume (pick one when VOLUME_ID is omitted)")
                .args_override_self(true)
                .arg(id())
                .arg(global())
                .arg(json()),
        )
        .subcommand(
            Command::new("create")
                .about("Create a network volume in one data center, or a global volume")
                .args_override_self(true)
                .arg(
                    Arg::new("name")
                        .long("name")
                        .required(true)
                        .help("Human-readable name"),
                )
                .arg(size())
                .arg(
                    Arg::new("region")
                        .long("region")
                        .action(ArgAction::Append)
                        .help("Region, ID prefix or data center; repeatable"),
                )
                .arg(
                    Arg::new("data-center")
                        .long("data-center")
                        .help("Exact data center ID"),
                )
                .arg(
                    Arg::new("type")
                        .long("type")
                        .value_parser(["standard", "high_performance"])
                        .ignore_case(true)
                        .help("Storage tier, immutable after creation"),
                )
                .arg(global())
                .arg(json()),
        )
        .subcommand(
            Command::new("update")
                .about("Rename and/or resize a volume")
                .args_override_self(true)
                .arg(id().required(true))
                .arg(Arg::new("name").long("name").help("New name"))
                .arg(size())
                .arg(global())
                .arg(json()),
        )
        .subcommand(
            Command::new("expand")
                .about("Grow a network volume; storage can never shrink")
                .args_override_self(true)
                .arg(id().required(true))
                .arg(
                    Arg::new("size")
                        .long("size")
                        .required(true)
                        .value_name("N|+N")
                        .allow_hyphen_values(true)
                        .help("New total size, or +N to add N GB"),
                )
                .arg(json()),
        )
        .subcommand(
            Command::new("delete")
                .about("Delete a volume and everything stored on it")
                .args_override_self(true)
                .arg(id().required(true))
                .arg(
                    Arg::new("yes")
                        .long("yes")
                        .short('y')
                        .action(ArgAction::SetTrue)
                        .help("Skip the confirmation prompt"),
                )
                .arg(global()),
        )
}
#[derive(Clone, Copy)]
enum Kind {
    Network,
    Global,
}
impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Global => "global",
        }
    }
    fn path(self) -> &'static str {
        match self {
            Self::Network => "/v2/network-volumes",
            Self::Global => "/v2/global-volumes",
        }
    }
    fn collection(self) -> &'static str {
        match self {
            Self::Network => "networkVolumes",
            Self::Global => "globalVolumes",
        }
    }
}
enum Placement {
    Global,
    Network {
        size: u32,
        regions: Vec<String>,
        data_center: Option<String>,
        tier: Option<String>,
    },
}
enum Action {
    List,
    Get {
        id: Option<String>,
    },
    Create {
        name: String,
        placement: Placement,
    },
    Update {
        id: String,
        name: Option<String>,
        size: Option<u32>,
    },
    Expand {
        id: String,
        growth: Growth,
    },
    Delete {
        id: String,
        yes: bool,
    },
}
enum Growth {
    Total(u64),
    Add(u64),
}
impl Growth {
    fn parse(size: &str) -> CliResult<Self> {
        let text = size.trim();
        let digits = text.strip_prefix('+').unwrap_or(text);
        let invalid = || {
            CliError::Usage(format!(
                "Invalid value for --size: '{size}' is not a size. use N for a total or +N to add N GB"
            ))
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        let number = digits.parse().map_err(|_| invalid())?;
        Ok(if text.starts_with('+') {
            Self::Add(number)
        } else {
            Self::Total(number)
        })
    }
    async fn resolve(
        &self,
        client: &Client,
        id: &str,
        require_growth: bool,
    ) -> CliResult<(u64, u64)> {
        let row = client
            .run_rest_request("GET", &format!("/v2/network-volumes/{id}"), None, &[])
            .await?;
        let current = field(&row, "size")?.as_u64().ok_or_else(|| {
            Error::InvalidResponse("volume size must be an unsigned integer".into())
        })?;
        let target = match *self {
            Self::Total(total) => total,
            Self::Add(extra) => current.checked_add(extra).ok_or_else(|| {
                CliError::Usage("requested size exceeds the integer limit".into())
            })?,
        };
        if target < current || (require_growth && target == current) {
            return Err(CliError::Usage(format!(
                "network volumes can only grow: {id} is {current} GB, requested {target} GB"
            )));
        }
        if target > 4096 {
            return Err(CliError::Usage(format!(
                "network volumes hold at most 4096 GB, requested {target} GB"
            )));
        }
        Ok((current, target))
    }
}
pub(super) struct Options {
    kind: Kind,
    json: bool,
    action: Action,
}
impl Options {
    pub(super) fn read(args: &ArgMatches) -> CliResult<Self> {
        let (name, args) = args
            .subcommand()
            .ok_or_else(|| CliError::Usage("a volume command is required".into()))?;
        let kind = if name != "expand" && args.get_flag("global") {
            Kind::Global
        } else {
            Kind::Network
        };
        let string = |key| args.get_one::<String>(key).cloned();
        let id = || string("id").expect("required by parser");
        let action = match name {
            "list" => Action::List,
            "get" => Action::Get { id: string("id") },
            "create" => {
                let size = args.get_one::<u32>("size").copied();
                let regions: Vec<String> = args
                    .get_many::<String>("region")
                    .into_iter()
                    .flatten()
                    .cloned()
                    .collect();
                let center = string("data-center").filter(|s| !s.is_empty());
                let tier = string("type").map(|s| s.to_uppercase());
                let placement = match kind {
                    Kind::Global => {
                        let given: Vec<_> = [
                            ("--size", size.is_some()),
                            ("--region", !regions.is_empty()),
                            ("--data-center", center.is_some()),
                            ("--type", tier.is_some()),
                        ]
                        .into_iter()
                        .filter_map(|(flag, given)| given.then_some(flag))
                        .collect();
                        if !given.is_empty() {
                            return Err(CliError::Usage(format!(
                                "{} do not apply to global volumes",
                                given.join(", ")
                            )));
                        }
                        Placement::Global
                    }
                    Kind::Network => {
                        let size = size.ok_or_else(|| {
                            CliError::Usage("pass --size in GB for a network volume".into())
                        })?;
                        if !regions.is_empty() && center.is_some() {
                            return Err(CliError::Usage(
                                "use either --region or --data-center".into(),
                            ));
                        }
                        Placement::Network {
                            size,
                            regions,
                            data_center: center,
                            tier,
                        }
                    }
                };
                Action::Create {
                    name: string("name").expect("required by parser"),
                    placement,
                }
            }
            "update" => {
                let size = args.get_one::<u32>("size").copied();
                let name = string("name");
                if matches!(kind, Kind::Global) && size.is_some() {
                    return Err(CliError::Usage(
                        "global volumes have no size to change".into(),
                    ));
                }
                if name.is_none() && size.is_none() {
                    return Err(CliError::Usage("pass --name and/or --size".into()));
                }
                Action::Update {
                    id: id(),
                    name,
                    size,
                }
            }
            "expand" => Action::Expand {
                id: id(),
                growth: Growth::parse(&string("size").expect("required by parser"))?,
            },
            "delete" => Action::Delete {
                id: id(),
                yes: args.get_flag("yes"),
            },
            _ => return Err(CliError::Usage("unknown volume command".into())),
        };
        Ok(Self {
            kind,
            json: name != "delete" && args.get_flag("json"),
            action,
        })
    }
    pub(super) async fn execute(
        &self,
        client: &Client,
        ui: &mut impl Interaction,
    ) -> CliResult<String> {
        match &self.action {
            Action::List => {
                let rows =
                    api::list_all(client, self.kind.path(), self.kind.collection(), &[]).await?;
                if self.json {
                    return Ok(output::json(&rows)?);
                }
                let columns = match self.kind {
                    Kind::Network => vec!["ID", "NAME", "SIZE", "DATA CENTER", "TYPE"],
                    Kind::Global => vec!["ID", "NAME", "CREATED"],
                };
                let cells = rows
                    .iter()
                    .map(|row| match self.kind {
                        Kind::Network => Ok(fields(row, self.kind)?
                            .into_iter()
                            .map(|(_, value)| value)
                            .collect()),
                        Kind::Global => Ok(vec![
                            string(row, "id")?.into(),
                            string(row, "name")?.into(),
                            string(row, "createdAt")?.chars().take(10).collect(),
                        ]),
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(output::table(
                    &columns.into_iter().map(str::to_owned).collect::<Vec<_>>(),
                    &cells,
                    &format!(
                        "no {} volumes. create one with rp volume create",
                        self.kind.name()
                    ),
                ))
            }
            Action::Get { id } => {
                let id = match id {
                    Some(id) => id.clone(),
                    None => ui.pick(
                        &format!("{} volume", self.kind.name()),
                        &choices(client, self.kind).await?,
                        "pass VOLUME_ID",
                    )?,
                };
                let row = client
                    .run_rest_request("GET", &format!("{}/{id}", self.kind.path()), None, &[])
                    .await?;
                self.show(&row, None)
            }
            Action::Create { name, placement } => {
                let mut body = json!({"name": name});
                if let Placement::Network {
                    size,
                    regions,
                    data_center,
                    tier,
                } = placement
                {
                    let center = match data_center {
                        Some(center) => center.clone(),
                        None => select_data_center(client, ui, regions, tier.as_deref()).await?,
                    };
                    body["size"] = (*size).into();
                    body["dataCenter"] = center.into();
                    if let Some(tier) = tier {
                        body["type"] = tier.clone().into();
                    }
                }
                let row = client
                    .run_rest_request("POST", self.kind.path(), Some(&body), &[])
                    .await?;
                self.show(&row, Some("created"))
            }
            Action::Update { id, name, size } => {
                let mut body = json!({});
                if let Some(name) = name {
                    body["name"] = name.clone().into();
                }
                if let Some(size) = size {
                    Growth::Total(u64::from(*size))
                        .resolve(client, id, false)
                        .await?;
                    body["size"] = (*size).into();
                }
                let row = client
                    .run_rest_request(
                        "PATCH",
                        &format!("{}/{id}", self.kind.path()),
                        Some(&body),
                        &[],
                    )
                    .await?;
                self.show(&row, Some("updated"))
            }
            Action::Expand { id, growth } => {
                let (current, target) = growth.resolve(client, id, true).await?;
                let row = client
                    .run_rest_request(
                        "PATCH",
                        &format!("/v2/network-volumes/{id}"),
                        Some(&json!({"size": target})),
                        &[],
                    )
                    .await?;
                if self.json {
                    Ok(output::json(&row)?)
                } else {
                    Ok(format!(
                        "✓ expanded {id} from {current} GB to {} GB\n",
                        output::cell(field(&row, "size")?)
                    ))
                }
            }
            Action::Delete { id, yes } => {
                let path = format!("{}/{id}", self.kind.path());
                if !yes {
                    let row = client.run_rest_request("GET", &path, None, &[]).await?;
                    ui.confirm(
                        &format!(
                            "delete {} volume {id} ({}) and all its data?",
                            self.kind.name(),
                            label(&row)?
                        ),
                        *yes,
                    )?;
                }
                client.run_rest_request("DELETE", &path, None, &[]).await?;
                Ok(format!("✓ deleted {} volume {id}\n", self.kind.name()))
            }
        }
    }
    fn show(&self, row: &Value, action: Option<&str>) -> CliResult<String> {
        if self.json {
            return Ok(output::json(row)?);
        }
        let mut text = if let Some(action) = action {
            format!(
                "✓ {action} {} volume {}\n",
                self.kind.name(),
                string(row, "id")?
            )
        } else {
            String::new()
        };
        text.push_str(&output::fields(&fields(row, self.kind)?));
        Ok(text)
    }
}

#[cfg(test)]
mod tests;
fn field<'a>(row: &'a Value, key: &str) -> Result<&'a Value> {
    row.get(key)
        .ok_or_else(|| Error::InvalidResponse(format!("volume is missing {key}")))
}
fn string<'a>(row: &'a Value, key: &str) -> Result<&'a str> {
    field(row, key)?
        .as_str()
        .ok_or_else(|| Error::InvalidResponse(format!("volume {key} must be a string")))
}
fn fields(row: &Value, kind: Kind) -> Result<Vec<(String, String)>> {
    let mut fields = vec![
        ("id".into(), string(row, "id")?.into()),
        ("name".into(), string(row, "name")?.into()),
    ];
    fields.extend(match kind {
        Kind::Network => vec![
            (
                "size".into(),
                format!("{} GB", output::cell(field(row, "size")?)),
            ),
            ("data center".into(), string(row, "dataCenter")?.into()),
            ("type".into(), string(row, "type")?.to_lowercase()),
        ],
        Kind::Global => vec![
            ("created".into(), string(row, "createdAt")?.into()),
            ("created by".into(), string(row, "createdByUserId")?.into()),
        ],
    });
    Ok(fields)
}
fn label(row: &Value) -> Result<String> {
    let name = string(row, "name")?;
    if row.get("size").is_none() {
        return Ok(name.into());
    }
    Ok(format!(
        "{name}  {} GB  {}  {}",
        output::cell(field(row, "size")?),
        string(row, "dataCenter")?,
        string(row, "type")?.to_lowercase()
    ))
}
async fn choices(client: &Client, kind: Kind) -> Result<Vec<(String, String)>> {
    api::list_all(client, kind.path(), kind.collection(), &[])
        .await?
        .iter()
        .map(|row| Ok((string(row, "id")?.into(), label(row)?)))
        .collect()
}
async fn select_data_center(
    client: &Client,
    ui: &mut impl Interaction,
    regions: &[String],
    tier: Option<&str>,
) -> CliResult<String> {
    let mut centers = client.list_catalog_datacenters(&[]).await?;
    if !regions.is_empty() {
        let allowed = compute::resolve_regions(regions.iter().map(String::as_str), &centers)?;
        centers.retain(|dc| {
            dc["id"]
                .as_str()
                .is_some_and(|id| allowed.iter().any(|v| v == id))
        });
    }
    let mut candidates = Vec::new();
    for center in &centers {
        let kinds = center["networkVolumeTypes"].as_array().ok_or_else(|| {
            Error::InvalidResponse("data center is missing networkVolumeTypes".into())
        })?;
        if kinds.is_empty() || tier.is_some_and(|tier| !kinds.iter().any(|v| v == tier)) {
            continue;
        }
        let kinds = kinds
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_lowercase)
                    .ok_or_else(|| Error::InvalidResponse("volume type must be a string".into()))
            })
            .collect::<Result<Vec<_>>>()?;
        let id = string(center, "id")?;
        let label = format!(
            "{id}  {}  {}",
            string(center, "region")?.to_lowercase().replace('_', " "),
            kinds.join(", ")
        );
        candidates.push((id.to_owned(), label));
    }
    let scope = if regions.is_empty() {
        "the catalog".into()
    } else {
        format!("region {}", regions.join(", "))
    };
    let offered = tier.map_or_else(
        || "network volumes".into(),
        |tier| format!("{} network volumes", tier.to_lowercase()),
    );
    match candidates.len() {
        0 => Err(Error::InvalidInput(format!("no data center in {scope} offers {offered}")).into()),
        1 => Ok(candidates.remove(0).0),
        _ => {
            let ids = candidates
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            ui.pick("data center", &candidates, &format!("several data centers in {scope} offer {offered} ({ids}). pass one with --data-center"))
        }
    }
}
