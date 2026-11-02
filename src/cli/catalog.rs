use super::output;
use crate::{
    Client, Error, Result, Value,
    compute::{self, GpuCatalogOptions},
};
use clap::{Arg, ArgAction, ArgMatches, Command, parser::ValueSource, value_parser};

pub(super) fn command() -> Command {
    Command::new("catalog")
        .about("Search GPU and CPU pricing and stock by data center")
        .args_override_self(true)
        .arg(Arg::new("query").value_name("QUERY"))
        .arg(
            Arg::new("pod")
                .long("pod")
                .action(ArgAction::SetTrue)
                .overrides_with("serverless")
                .help("Pod pricing and stock (default)"),
        )
        .arg(
            Arg::new("serverless")
                .long("serverless")
                .action(ArgAction::SetTrue)
                .overrides_with("pod")
                .help("Serverless pricing, pools, and stock"),
        )
        .arg(
            Arg::new("cpu")
                .long("cpu")
                .action(ArgAction::SetTrue)
                .help("Search CPU flavors instead of GPUs"),
        )
        .arg(
            Arg::new("datacenters")
                .long("datacenters")
                .action(ArgAction::SetTrue)
                .help("List data centers and their regions"),
        )
        .arg(
            Arg::new("count")
                .long("count")
                .default_value("1")
                .value_parser(value_parser!(u32).range(1..))
                .help("GPUs per pod or worker"),
        )
        .arg(
            Arg::new("vcpu")
                .long("vcpu")
                .value_parser(value_parser!(u32).range(2..))
                .help("vCPUs per pod or worker (with --cpu)"),
        )
        .arg(
            Arg::new("region")
                .long("region")
                .action(ArgAction::Append)
                .help("Region, id prefix, or data center id; repeatable"),
        )
        .arg(
            Arg::new("cloud")
                .long("cloud")
                .value_parser(["secure", "community"])
                .ignore_case(true)
                .help("Pod cloud tier"),
        )
        .arg(
            Arg::new("cuda")
                .long("cuda")
                .action(ArgAction::Append)
                .help("CUDA floor (>=12.4) or exact versions (12.4,12.8); repeatable"),
        )
        .arg(
            Arg::new("min-vram")
                .long("min-vram")
                .value_parser(value_parser!(i32))
                .allow_negative_numbers(true)
                .help("Minimum VRAM per GPU in GB"),
        )
        .arg(
            Arg::new("max-price")
                .long("max-price")
                .value_parser(value_parser!(f64))
                .allow_negative_numbers(true)
                .help("Maximum USD per GPU-hour"),
        )
        .arg(
            Arg::new("available")
                .long("available")
                .action(ArgAction::SetTrue)
                .help("Only show hardware that is in stock"),
        )
        .arg(
            Arg::new("json")
                .long("json")
                .action(ArgAction::SetTrue)
                .help("Print JSON for scripts and agents"),
        )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Gpu,
    Cpu,
    DataCenters,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Product {
    Pod,
    Serverless,
}
impl Product {
    fn api(self) -> &'static str {
        match self {
            Self::Pod => "POD",
            Self::Serverless => "SERVERLESS",
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cloud {
    Secure,
    Community,
}
impl Cloud {
    fn key(self) -> &'static str {
        match self {
            Self::Secure => "secure",
            Self::Community => "community",
        }
    }
}
pub(super) struct Options {
    mode: Mode,
    product: Product,
    query: Option<String>,
    count: u32,
    vcpu: Option<u32>,
    regions: Vec<String>,
    cloud: Option<Cloud>,
    cuda: Vec<String>,
    min_vram: Option<i32>,
    max_price: Option<f64>,
    available: bool,
    json: bool,
}
impl Options {
    pub(super) fn read(args: &ArgMatches) -> std::result::Result<Self, String> {
        let explicit = |name| args.value_source(name) == Some(ValueSource::CommandLine);
        if args.get_flag("cpu") && args.get_flag("datacenters") {
            return Err("use either --cpu or --datacenters".into());
        }
        let mode = if args.get_flag("datacenters") {
            Mode::DataCenters
        } else if args.get_flag("cpu") {
            Mode::Cpu
        } else {
            Mode::Gpu
        };
        let product = if args.get_flag("serverless") {
            Product::Serverless
        } else {
            Product::Pod
        };
        let (label, unsupported): (_, &[&str]) = match mode {
            Mode::DataCenters => (
                "--datacenters",
                &[
                    "product",
                    "count",
                    "vcpu",
                    "cloud",
                    "cuda",
                    "min-vram",
                    "max-price",
                    "available",
                ],
            ),
            Mode::Cpu => ("--cpu", &["count", "cuda", "min-vram", "max-price"]),
            Mode::Gpu => ("GPU", &["vcpu"]),
        };
        for &name in unsupported {
            let flag = if name == "product" {
                if product == Product::Pod {
                    "pod"
                } else {
                    "serverless"
                }
            } else {
                name
            };
            if explicit(flag) {
                return Err(format!("--{flag} cannot be used in {label} mode"));
            }
        }
        let cloud = args.get_one::<String>("cloud").map(|s| {
            if s.eq_ignore_ascii_case("secure") {
                Cloud::Secure
            } else {
                Cloud::Community
            }
        });
        if cloud.is_some() && product == Product::Serverless {
            return Err("--cloud applies to pods only".into());
        }
        if mode == Mode::Cpu && cloud == Some(Cloud::Community) {
            return Err("--cpu is only available in the secure cloud".into());
        }
        let many = |name| {
            args.get_many::<String>(name)
                .into_iter()
                .flatten()
                .cloned()
                .collect()
        };
        Ok(Self {
            mode,
            product,
            cloud,
            query: args
                .get_one::<String>("query")
                .filter(|s| !s.is_empty())
                .cloned(),
            count: *args.get_one("count").expect("count has a default"),
            vcpu: args.get_one("vcpu").copied(),
            regions: many("region"),
            cuda: many("cuda"),
            min_vram: args.get_one("min-vram").copied(),
            max_price: args.get_one("max-price").copied(),
            available: args.get_flag("available"),
            json: args.get_flag("json"),
        })
    }
    pub(super) async fn execute(&self) -> Result<String> {
        let rows = match self.mode {
            Mode::Gpu => self.gpu_rows().await?,
            Mode::Cpu => self.cpu_rows(&Client::from_env()?).await?,
            Mode::DataCenters => self.datacenter_rows(&Client::from_env()?).await?,
        };
        if self.json {
            return output::json(&rows);
        }
        let (headers, cells, empty) = match self.mode {
            Mode::Gpu => (
                self.gpu_headers(),
                rows.iter()
                    .map(|r| self.gpu_cells(r))
                    .collect::<Result<Vec<_>>>()?,
                "no gpus match",
            ),
            Mode::Cpu => (
                vec![
                    "CPU",
                    "NAME",
                    "GROUP",
                    "VCPU",
                    "RAM/VCPU",
                    "$/VCPU-HR",
                    "STOCK",
                    "DATA CENTERS",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
                rows.iter()
                    .map(|r| self.cpu_cells(r))
                    .collect::<Result<Vec<_>>>()?,
                "no cpu flavors match",
            ),
            Mode::DataCenters => (
                vec![
                    "DATA CENTER",
                    "REGION",
                    "VOLUME TYPES",
                    "GLOBAL NETWORK",
                    "COMPLIANCE",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
                rows.iter()
                    .map(datacenter_cells)
                    .collect::<Result<Vec<_>>>()?,
                "no data centers match",
            ),
        };
        Ok(output::table(&headers, &cells, empty))
    }
    async fn gpu_rows(&self) -> Result<Vec<Value>> {
        let (minimum, exact) = compute::parse_cuda(self.cuda.iter().map(String::as_str))?;
        let client = Client::from_env()?;
        let mut rows = client
            .list_catalog_gpus(&GpuCatalogOptions {
                product: Some(self.product.api().into()),
                count: Some(u64::from(self.count)),
                cloud: self.cloud.map(|c| c.key().to_uppercase()),
                cuda_versions: exact,
                min_cuda_version: minimum,
            })
            .await?;
        if let Some(query) = &self.query {
            rows = compute::resolve_gpus(query, &rows)?
                .into_iter()
                .cloned()
                .collect();
        }
        if self.product == Product::Serverless {
            rows.retain(|r| {
                r.get("pool")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty())
            });
        }
        let mut selected = Vec::new();
        for mut row in self.filter_stock(&client, rows).await? {
            let price = self.unit_price(&row)?;
            if let Some(min) = self.min_vram.filter(|min| *min != 0) {
                let memory = field(&row, "memory")?
                    .as_f64()
                    .ok_or_else(|| invalid("memory must be numeric"))?;
                if memory < f64::from(min) {
                    continue;
                }
            }
            if self
                .max_price
                .is_some_and(|max| price.is_none_or(|p| p > max))
            {
                continue;
            }
            let name = text(&row, "name")?.to_owned();
            let totals: serde_json::Map<String, Value> = object(&row, "price")?
                .iter()
                .map(|(tier, value)| {
                    let total = optional_number(value)?.map(|price| {
                        format!("{:.4}", price * f64::from(self.count))
                            .parse::<f64>()
                            .expect("formatted number")
                    });
                    Ok((tier.clone(), total.map_or(Value::Null, Value::from)))
                })
                .collect::<Result<_>>()?;
            row["count"] = self.count.into();
            row["totalPrice"] = totals.into();
            selected.push((rank(&row), price.unwrap_or(f64::INFINITY), name, row));
        }
        selected.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.total_cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        });
        Ok(selected.into_iter().map(|(_, _, _, row)| row).collect())
    }
    fn unit_price(&self, row: &Value) -> Result<Option<f64>> {
        let prices = object(row, "price")?;
        if self.product == Product::Serverless {
            return optional_number(prices.get("serverless").unwrap_or(&Value::Null));
        }
        if let Some(cloud) = self.cloud {
            return optional_number(prices.get(cloud.key()).unwrap_or(&Value::Null));
        }
        let mut offered = Vec::new();
        for tier in ["secure", "community"] {
            if let Some(price) = optional_number(prices.get(tier).unwrap_or(&Value::Null))? {
                offered.push(price);
            }
        }
        Ok(offered.into_iter().min_by(f64::total_cmp))
    }
    async fn cpu_rows(&self, client: &Client) -> Result<Vec<Value>> {
        let mut rows = client
            .list_catalog_cpus(Some(self.product.api()), self.vcpu.map(u64::from))
            .await?;
        if let Some(query) = &self.query {
            rows = compute::match_cpus(query, &rows)?
                .into_iter()
                .cloned()
                .collect();
            if rows.is_empty() {
                return Err(Error::InvalidInput(format!("unknown cpu '{query}'")));
            }
        }
        let mut rows = self.filter_stock(client, rows).await?;
        rows.sort_by_key(|r| std::cmp::Reverse(rank(r)));
        Ok(rows)
    }
    async fn datacenter_rows(&self, client: &Client) -> Result<Vec<Value>> {
        let mut rows = client.list_catalog_datacenters(&[]).await?;
        if !self.regions.is_empty() || self.query.is_some() {
            let allowed = compute::resolve_regions(
                self.regions
                    .iter()
                    .chain(self.query.iter())
                    .map(String::as_str),
                &rows,
            )?;
            rows.retain(|r| {
                r["id"]
                    .as_str()
                    .is_some_and(|id| allowed.iter().any(|a| a == id))
            });
        }
        Ok(rows)
    }
    async fn filter_stock(&self, client: &Client, rows: Vec<Value>) -> Result<Vec<Value>> {
        let allowed = if self.regions.is_empty() {
            None
        } else {
            Some(compute::resolve_regions(
                self.regions.iter().map(String::as_str),
                &client.list_catalog_datacenters(&[]).await?,
            )?)
        };
        let mut selected = Vec::new();
        for mut row in rows {
            if !row.is_object() {
                return Err(invalid("record must be an object"));
            }
            if let Some(allowed) = &allowed {
                let centers: Vec<_> = data_centers(&row)?
                    .iter()
                    .filter(|dc| {
                        dc["id"]
                            .as_str()
                            .is_some_and(|id| allowed.iter().any(|a| a == id))
                    })
                    .cloned()
                    .collect();
                row["availability"] = compute::best_availability(
                    centers.iter().map(|dc| dc["availability"].as_str()),
                )
                .into();
                row["dataCenters"] = centers.into();
            }
            if !self.available || rank(&row) > 0 {
                selected.push(row);
            }
        }
        Ok(selected)
    }
    fn gpu_headers(&self) -> Vec<String> {
        let mut headers = vec!["GPU".into(), "VRAM".into()];
        let suffix = if self.count > 1 {
            format!(" x{}", self.count)
        } else {
            String::new()
        };
        if self.product == Product::Serverless {
            headers.extend(["POOL".into(), format!("$/HR{suffix}")]);
        } else {
            headers.extend(
                self.tiers()
                    .iter()
                    .map(|tier| format!("{} $/HR{suffix}", tier.to_uppercase())),
            );
            headers.push("MAX".into());
        }
        headers.extend(["STOCK".into(), "DATA CENTERS".into()]);
        headers
    }
    fn tiers(&self) -> Vec<&'static str> {
        self.cloud
            .map_or_else(|| vec!["secure", "community"], |cloud| vec![cloud.key()])
    }
    fn gpu_cells(&self, row: &Value) -> Result<Vec<String>> {
        let mut cells = vec![
            text(row, "name")?.into(),
            format!("{}GB", output::cell(field(row, "memory")?)),
        ];
        let totals = object(row, "totalPrice")?;
        if self.product == Product::Serverless {
            cells.push(text(row, "pool")?.into());
            cells.push(money(totals.get("serverless"), None)?);
        } else {
            for tier in self.tiers() {
                cells.push(money(totals.get(tier), None)?);
            }
            let counts = object(row, "maxCount")?;
            let maximum = if let Some(cloud) = self.cloud {
                counts.get(cloud.key())
            } else {
                counts.values().max_by_key(|v| v.as_u64())
            };
            cells.push(output::cell(maximum.unwrap_or(&Value::Null)));
        }
        cells.extend(stock_cells(row)?);
        Ok(cells)
    }
    fn cpu_cells(&self, row: &Value) -> Result<Vec<String>> {
        let tier = if self.product == Product::Serverless {
            "serverlessPerVcpu"
        } else {
            "securePerVcpu"
        };
        let vcpu = field(row, "vcpu")?;
        let ram = field(row, "ramGbPerVcpu")?
            .as_f64()
            .ok_or_else(|| invalid("ramGbPerVcpu must be numeric"))?;
        let mut cells = vec![
            text(row, "id")?.into(),
            text(row, "name")?.into(),
            text(row, "group")?.into(),
            format!(
                "{}-{}",
                output::cell(field(vcpu, "min")?),
                output::cell(field(vcpu, "max")?)
            ),
            format!("{}GB", output::general(ram, 6)),
            money(object(row, "price")?.get(tier), Some(4))?,
        ];
        cells.extend(stock_cells(row)?);
        Ok(cells)
    }
}
fn invalid(message: &str) -> Error {
    Error::InvalidResponse(format!("catalog: {message}"))
}
fn field<'a>(row: &'a Value, key: &str) -> Result<&'a Value> {
    row.get(key)
        .ok_or_else(|| invalid(&format!("missing {key}")))
}
fn text<'a>(row: &'a Value, key: &str) -> Result<&'a str> {
    field(row, key)?
        .as_str()
        .ok_or_else(|| invalid(&format!("{key} must be a string")))
}
fn object<'a>(row: &'a Value, key: &str) -> Result<&'a serde_json::Map<String, Value>> {
    field(row, key)?
        .as_object()
        .ok_or_else(|| invalid(&format!("{key} must be an object")))
}
fn optional_number(value: &Value) -> Result<Option<f64>> {
    if value.is_null() {
        return Ok(None);
    }
    let number = value
        .as_f64()
        .ok_or_else(|| invalid("price must be numeric or null"))?;
    Ok((number != 0.0).then_some(number))
}
fn rank(row: &Value) -> u8 {
    compute::availability_rank(row["availability"].as_str())
}
fn data_centers(row: &Value) -> Result<&[Value]> {
    match row.get("dataCenters") {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(values)) => Ok(values),
        Some(_) => Err(invalid("dataCenters must be an array")),
    }
}
fn stock_cells(row: &Value) -> Result<[String; 2]> {
    let mut centers = data_centers(row)?
        .iter()
        .map(|dc| Ok((text(dc, "id")?, text(dc, "availability")?)))
        .collect::<Result<Vec<_>>>()?;
    centers.sort_by_key(|(_, stock)| std::cmp::Reverse(compute::availability_rank(Some(stock))));
    let stock = row["availability"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("NONE")
        .to_lowercase();
    let centers = centers
        .iter()
        .map(|(id, stock)| format!("{id} {}", stock.to_lowercase()))
        .collect::<Vec<_>>()
        .join(", ");
    Ok([
        stock,
        if centers.is_empty() {
            "-".into()
        } else {
            centers
        },
    ])
}
fn money(value: Option<&Value>, significant: Option<usize>) -> Result<String> {
    Ok(optional_number(value.unwrap_or(&Value::Null))?.map_or_else(
        || "-".into(),
        |value| {
            format!(
                "${}",
                significant.map_or_else(
                    || format!("{value:.2}"),
                    |digits| output::general(value, digits)
                )
            )
        },
    ))
}
fn datacenter_cells(row: &Value) -> Result<Vec<String>> {
    let kinds = field(row, "networkVolumeTypes")?
        .as_array()
        .ok_or_else(|| invalid("networkVolumeTypes must be an array"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(|s| Value::String(s.to_lowercase()))
                .ok_or_else(|| invalid("volume type must be a string"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(vec![
        text(row, "id")?.into(),
        text(row, "region")?.to_lowercase().replace('_', " "),
        output::cell(&Value::Array(kinds)),
        output::cell(field(row, "globalNetwork")?),
        output::cell(field(row, "compliance")?),
    ])
}
