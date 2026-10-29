//! Shared resolution of named platform resources, with IDs taking precedence.
use crate::{Error, Result, Value, client::required_str};

/// Finds an exact ID or a unique name while preserving the source collection's ordering.
///
/// # Errors
/// Returns an actionable ambiguity error listing matching IDs, or malformed response errors.
pub fn find_by_id_or_name<'items>(
    items: &'items [Value],
    reference: &str,
    noun: &str,
) -> Result<Option<&'items Value>> {
    for item in items {
        if required_str(item, "id")? == reference {
            return Ok(Some(item));
        }
    }
    let matches: Vec<_> = items
        .iter()
        .filter(|v| v.get("name").and_then(Value::as_str) == Some(reference))
        .collect();
    if matches.len() > 1 {
        let ids = matches
            .iter()
            .map(|v| required_str(v, "id"))
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        return Err(Error::InvalidInput(format!(
            "multiple {noun} named '{reference}' ({ids}); reference by id"
        )));
    }
    Ok(matches.first().copied())
}
