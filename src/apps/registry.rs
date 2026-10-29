//! Resolves private-image registry credentials during provisioning.
use super::{AppsApiClient, lookup::find_by_id_or_name};
use crate::{Error, Result, client::required_str};

/// Resolves an optional credential name or ID to its backend identifier.
///
/// # Errors
/// Returns request errors, ambiguity, or a missing-credential error with available names.
pub async fn resolve_registry_auth(
    name: Option<&str>,
    api: &AppsApiClient,
) -> Result<Option<String>> {
    let Some(name) = name.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let credentials = api.list_registry_auths().await?;
    if let Some(record) = find_by_id_or_name(&credentials, name, "registry credentials")? {
        return Ok(Some(required_str(record, "id")?.into()));
    }
    let mut available = credentials
        .iter()
        .map(|c| required_str(c, "name"))
        .collect::<Result<Vec<_>>>()?;
    available.sort();
    let available = if available.is_empty() {
        "(none)".into()
    } else {
        available.join(", ")
    };
    Err(Error::InvalidInput(format!(
        "registry credential '{name}' not found. available: {available}. create one with `rp registry add {name}`"
    )))
}
