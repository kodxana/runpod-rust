//! Live catalog queries and human-readable GPU, CPU, region, and CUDA selection.
use crate::{Client, Error, Result, Value};

/// A user-supplied hardware, region, or CUDA selection could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ResolutionError {
    message: String,
}
pub(crate) fn resolution(message: String) -> Error {
    ResolutionError { message }.into()
}

/// Optional GPU catalog context. Without a nonempty product, stock filters are omitted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GpuCatalogOptions {
    /// Product context, normally `POD` or `SERVERLESS`.
    pub product: Option<String>,
    /// GPUs per pod or worker.
    pub count: Option<u64>,
    /// Cloud tier, normally `SECURE` or `COMMUNITY`.
    pub cloud: Option<String>,
    /// Exact CUDA versions, sent as one comma-separated query value.
    pub cuda_versions: Vec<String>,
    /// Minimum CUDA version.
    pub min_cuda_version: Option<String>,
}
impl Client {
    /// Lists GPU catalog records, retaining all server-provided fields.
    ///
    /// # Errors
    /// Returns transport/API errors or an invalid collection response.
    pub async fn list_catalog_gpus(&self, options: &GpuCatalogOptions) -> Result<Vec<Value>> {
        let mut params = product_params(options.product.as_deref());
        if options.product.as_ref().is_some_and(|s| !s.is_empty()) {
            if let Some(count) = options.count {
                params.push(("count", count.to_string()));
            }
            if let Some(cloud) = options.cloud.as_deref() {
                params.push(("cloud", cloud.into()));
            }
            if !options.cuda_versions.is_empty() {
                params.push(("cudaVersions", options.cuda_versions.join(",")));
            }
            if let Some(minimum) = options.min_cuda_version.as_deref() {
                params.push(("minCudaVersion", minimum.into()));
            }
        }
        catalog(self, "/v2/catalog/gpus", "gpus", &params).await
    }
    /// Lists CPU flavors, optionally with product availability and vCPU count.
    ///
    /// # Errors
    /// Returns transport/API errors or an invalid collection response.
    pub async fn list_catalog_cpus(
        &self,
        product: Option<&str>,
        vcpu_count: Option<u64>,
    ) -> Result<Vec<Value>> {
        let mut params = product_params(product);
        if product.is_some_and(|s| !s.is_empty())
            && let Some(count) = vcpu_count
        {
            params.push(("vcpuCount", count.to_string()));
        }
        catalog(self, "/v2/catalog/cpus", "cpus", &params).await
    }
    /// Lists data centers with optional comma-separated include flags.
    ///
    /// # Errors
    /// Returns transport/API errors or an invalid collection response.
    pub async fn list_catalog_datacenters(&self, include: &[&str]) -> Result<Vec<Value>> {
        let params = if include.is_empty() {
            vec![]
        } else {
            vec![("include", include.join(","))]
        };
        catalog(self, "/v2/catalog/datacenters", "dataCenters", &params).await
    }
}
fn product_params(product: Option<&str>) -> Vec<(&'static str, String)> {
    let mut params = Vec::new();
    if product.is_some_and(|s| !s.is_empty()) {
        params.push(("include", "AVAILABILITY".into()));
    }
    if let Some(product) = product {
        params.push(("product", product.into()));
    }
    params
}
async fn catalog(
    client: &Client,
    path: &str,
    collection: &str,
    params: &[(&str, String)],
) -> Result<Vec<Value>> {
    let params: Vec<_> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let mut data = client.run_rest_request("GET", path, None, &params).await?;
    match data
        .as_object_mut()
        .and_then(|object| object.remove(collection))
    {
        Some(Value::Array(rows)) => Ok(rows),
        _ => Err(Error::InvalidResponse(format!(
            "catalog is missing array {collection}"
        ))),
    }
}
fn required<'a>(row: &'a Value, key: &str) -> Result<&'a str> {
    row.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidResponse(format!("catalog record is missing string {key}")))
}
fn optional<'a>(row: &'a Value, key: &str) -> Result<&'a str> {
    match row.get(key) {
        None | Some(Value::Null) => Ok(""),
        Some(Value::String(value)) => Ok(value),
        _ => Err(Error::InvalidResponse(format!(
            "catalog record has invalid {key}"
        ))),
    }
}
fn separator(character: char) -> bool {
    character.is_whitespace() || matches!(character, '_' | '-')
}
fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(separator)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn compact(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| !separator(*c))
        .collect()
}

struct Gpu<'a> {
    row: &'a Value,
    id: &'a str,
    name: &'a str,
    pool: &'a str,
}
impl<'a> Gpu<'a> {
    fn read(row: &'a Value) -> Result<Self> {
        Ok(Self {
            row,
            id: required(row, "id")?,
            name: optional(row, "name")?,
            pool: optional(row, "pool")?,
        })
    }
}
/// Matches exact IDs/names, then pools, tokens, and compact ID substrings, in catalog order.
///
/// # Errors
/// Returns an invalid-response error for malformed catalog records.
pub fn match_gpus<'a>(query: &str, gpus: &'a [Value]) -> Result<Vec<&'a Value>> {
    let gpus: Vec<_> = gpus.iter().map(Gpu::read).collect::<Result<_>>()?;
    let text = query.trim().to_lowercase();
    let exact: Vec<_> = gpus
        .iter()
        .filter(|gpu| text == gpu.id.to_lowercase() || text == gpu.name.to_lowercase())
        .map(|g| g.row)
        .collect();
    if !exact.is_empty() {
        return Ok(exact);
    }
    let pools: Vec<_> = gpus
        .iter()
        .filter(|gpu| gpu.pool.to_lowercase() == text)
        .map(|g| g.row)
        .collect();
    if !pools.is_empty() {
        return Ok(pools);
    }
    let wanted: Vec<_> = tokens(&text)
        .into_iter()
        .filter(|s| !matches!(s.as_str(), "nvidia" | "geforce" | "amd" | "instinct"))
        .collect();
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let hits: Vec<_> = gpus
        .iter()
        .filter(|gpu| {
            let mut haystack = tokens(gpu.id);
            haystack.extend(tokens(gpu.name));
            wanted.iter().all(|word| haystack.contains(word))
        })
        .map(|g| g.row)
        .collect();
    if !hits.is_empty() {
        return Ok(hits);
    }
    let needle = wanted.concat();
    Ok(gpus
        .iter()
        .filter(|gpu| compact(gpu.id).contains(&needle))
        .map(|g| g.row)
        .collect())
}
/// Resolves GPUs, listing known names when the query has no match.
///
/// # Errors
/// Returns [`ResolutionError`] through [`Error::Compute`] or malformed-record errors.
pub fn resolve_gpus<'a>(query: &str, gpus: &'a [Value]) -> Result<Vec<&'a Value>> {
    let matches = match_gpus(query, gpus)?;
    if matches.is_empty() {
        let mut known = gpus
            .iter()
            .map(|row| {
                let name = optional(row, "name")?;
                if name.is_empty() {
                    required(row, "id")
                } else {
                    Ok(name)
                }
            })
            .collect::<Result<Vec<_>>>()?;
        known.sort_unstable();
        return Err(resolution(format!(
            "unknown gpu '{query}'. known gpus: {}",
            known.join(", ")
        )));
    }
    Ok(matches)
}
/// Splits a GPU name and optional positive ASCII count, using the final colon.
///
/// # Errors
/// Returns a resolution error for malformed, zero, or overflowing `u64` counts.
pub fn parse_gpu_spec(value: &str) -> Result<(String, Option<u64>)> {
    let Some((name, count)) = value.trim().rsplit_once(':') else {
        return Ok((value.trim().into(), None));
    };
    let parsed = count.parse::<u64>().ok().filter(|n| *n > 0);
    if name.is_empty()
        || count.is_empty()
        || !count.bytes().all(|b| b.is_ascii_digit())
        || parsed.is_none()
    {
        return Err(resolution(format!(
            "invalid gpu '{value}'. use a name with an optional count, like h100:2"
        )));
    }
    Ok((name.into(), parsed))
}
/// Parses CUDA floors or exact versions, retaining order, duplicates, and spelling.
///
/// # Errors
/// Returns a resolution error for malformed versions, mixed modes, or components exceeding `u64`.
pub fn parse_cuda<'a>(
    values: impl IntoIterator<Item = &'a str>,
) -> Result<(Option<String>, Vec<String>)> {
    let mut minimum: Option<((u64, u64), String)> = None;
    let mut exact = Vec::new();
    for value in values {
        for part in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let floor = part.strip_prefix(">=");
            let version = floor.map_or(part, str::trim);
            let parsed = version
                .split_once('.')
                .and_then(|(major, minor)| {
                    if major.is_empty()
                        || minor.is_empty()
                        || !major
                            .bytes()
                            .chain(minor.bytes())
                            .all(|b| b.is_ascii_digit())
                    {
                        return None;
                    }
                    Some((major.parse::<u64>().ok()?, minor.parse::<u64>().ok()?))
                })
                .ok_or_else(|| {
                    resolution(format!(
                        "invalid cuda version '{part}'. use 12.4, 12.4,12.8, or '>=12.4'"
                    ))
                })?;
            if floor.is_some() {
                if minimum.as_ref().is_none_or(|(prior, _)| parsed > *prior) {
                    minimum = Some((parsed, version.into()));
                }
            } else {
                exact.push(version.into());
            }
        }
    }
    if minimum.is_some() && !exact.is_empty() {
        return Err(resolution(
            "use either a cuda floor (>=) or exact versions, not both".into(),
        ));
    }
    Ok((minimum.map(|(_, version)| version), exact))
}
/// Returns serverless pools and excluded device IDs selecting exactly the matched GPUs.
///
/// # Errors
/// Returns a resolution error if no matched GPU offers a pool, or malformed-record errors.
pub fn endpoint_gpu_selection<'a>(
    matches: impl IntoIterator<Item = &'a Value>,
    gpus: &[Value],
) -> Result<(Vec<String>, Vec<String>)> {
    let selected: Vec<_> = matches.into_iter().map(Gpu::read).collect::<Result<_>>()?;
    let mut pools = Vec::new();
    for gpu in &selected {
        if !gpu.pool.is_empty() && !pools.iter().any(|p| p == gpu.pool) {
            pools.push(gpu.pool.to_owned());
        }
    }
    if pools.is_empty() {
        let names: Vec<_> = selected
            .iter()
            .map(|g| if g.name.is_empty() { g.id } else { g.name })
            .collect();
        return Err(resolution(format!(
            "{} is not offered on serverless",
            names.join(", ")
        )));
    }
    let mut excluded = Vec::new();
    for gpu in gpus.iter().map(Gpu::read) {
        let gpu = gpu?;
        if pools.iter().any(|p| p == gpu.pool) && !selected.iter().any(|chosen| chosen.id == gpu.id)
        {
            excluded.push(gpu.id.into());
        }
    }
    Ok((pools, excluded))
}
fn region_alias(value: &str) -> Option<&'static str> {
    match compact(value).as_str() {
        "na" | "northamerica" => Some("NORTH_AMERICA"),
        "sa" | "southamerica" => Some("SOUTH_AMERICA"),
        "eu" | "europe" => Some("EUROPE"),
        "asia" | "apac" => Some("ASIA"),
        "me" | "middleeast" => Some("MIDDLE_EAST"),
        "africa" => Some("AFRICA"),
        "oceania" => Some("OCEANIA"),
        _ => None,
    }
}
/// Resolves region aliases, ID prefixes, or data-center IDs with ordered deduplication.
///
/// # Errors
/// Returns a resolution error for any unmatched value, or malformed-record errors.
pub fn resolve_regions<'a>(
    values: impl IntoIterator<Item = &'a str>,
    datacenters: &[Value],
) -> Result<Vec<String>> {
    let rows: Vec<_> = datacenters
        .iter()
        .map(|dc| Ok((required(dc, "id")?, optional(dc, "region")?)))
        .collect::<Result<_>>()?;
    let mut resolved = Vec::new();
    for value in values {
        let text = value.trim().to_uppercase().replace('_', "-");
        let prefix = format!("{text}-");
        let region = region_alias(value);
        let matches: Vec<_> = rows
            .iter()
            .filter(|(id, r)| {
                id.to_uppercase() == text
                    || id.to_uppercase().starts_with(&prefix)
                    || region == Some(*r)
            })
            .map(|(id, _)| *id)
            .collect();
        if matches.is_empty() {
            let mut ids: Vec<_> = rows.iter().map(|(id, _)| *id).collect();
            ids.sort_unstable();
            let names: std::collections::BTreeSet<_> = rows
                .iter()
                .filter(|(_, r)| !r.is_empty())
                .map(|(_, r)| r.to_lowercase().replace('_', "-"))
                .collect();
            return Err(resolution(format!(
                "unknown region '{value}'. use a region (eu, na, {}), an id prefix like us or eu-ro, or a data center ({})",
                names.into_iter().collect::<Vec<_>>().join(", "),
                ids.join(", ")
            )));
        }
        for id in matches {
            if !resolved.iter().any(|r| r == id) {
                resolved.push(id.to_owned());
            }
        }
    }
    Ok(resolved)
}
/// Matches CPU IDs, groups, and names, including names with equivalent separators.
///
/// # Errors
/// Returns an invalid-response error for malformed records.
pub fn match_cpus<'a>(query: &str, cpus: &'a [Value]) -> Result<Vec<&'a Value>> {
    let text = query.trim().to_lowercase();
    let mut matches = Vec::new();
    for cpu in cpus {
        let id = required(cpu, "id")?;
        let name = optional(cpu, "name")?;
        let group = optional(cpu, "group")?;
        if [id, name, group].iter().any(|s| s.to_lowercase() == text)
            || compact(&text) == compact(name)
        {
            matches.push(cpu);
        }
    }
    Ok(matches)
}
/// Resolves exactly one CPU flavor, distinguishing unknown and ambiguous queries.
///
/// # Errors
/// Returns a resolution error unless exactly one flavor matches, or malformed-record errors.
pub fn resolve_cpu<'a>(query: &str, cpus: &'a [Value]) -> Result<&'a Value> {
    let matches = match_cpus(query, cpus)?;
    if let [cpu] = matches.as_slice() {
        return Ok(cpu);
    }
    let mut known = cpus
        .iter()
        .map(|cpu| required(cpu, "id"))
        .collect::<Result<Vec<_>>>()?;
    known.sort_unstable();
    let problem = if matches.is_empty() {
        "unknown"
    } else {
        "ambiguous"
    };
    Err(resolution(format!(
        "{problem} cpu '{query}'. cpu flavors: {}",
        known.join(", ")
    )))
}
/// Ranks stock from zero through three; unknown or absent values have rank zero.
#[must_use]
pub fn availability_rank(level: Option<&str>) -> u8 {
    match level {
        Some("LOW") => 1,
        Some("MEDIUM") => 2,
        Some("HIGH") => 3,
        _ => 0,
    }
}
/// Returns the first highest-ranked stock value, or `NONE` for empty/absent values.
pub fn best_availability<'a>(levels: impl IntoIterator<Item = Option<&'a str>>) -> &'a str {
    let mut levels = levels.into_iter();
    let mut best = levels.next().flatten();
    for level in levels {
        if availability_rank(level) > availability_rank(best) {
            best = level;
        }
    }
    best.filter(|s| !s.is_empty()).unwrap_or("NONE")
}
