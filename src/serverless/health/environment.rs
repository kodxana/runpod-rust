use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(super) fn flag(values: &BTreeMap<String, String>, name: &str) -> bool {
    values.get(name).is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}
/// Recognize production serverless configuration, excluding platform test mode.
pub fn is_serverless_environment(values: &BTreeMap<String, String>) -> bool {
    ["RUNPOD_ENDPOINT_ID", "RUNPOD_WEBHOOK_GET_JOB"]
        .iter()
        .all(|k| values.get(*k).is_some_and(|v| !v.trim().is_empty()))
        && !flag(values, "RUNPOD_TEST")
}
/// Whether early probes are eligible; args excludes the executable name.
pub fn is_early_check_eligible(values: &BTreeMap<String, String>, args: &[String]) -> bool {
    let realtime = values
        .get("RUNPOD_REALTIME_PORT")
        .map_or("0", String::as_str)
        .trim();
    matches!(realtime, "" | "0")
        && is_serverless_environment(values)
        && !args.iter().any(|arg| {
            matches!(
                arg.split('=').next(),
                Some("--test_input" | "--rp_serve_api")
            )
        })
}
/// Locate an existing binary file, trying its environment override before package contents.
/// Missing or directory-valued overrides fall back to the package directory.
pub fn get_binary_path(
    name: &str,
    values: &BTreeMap<String, String>,
    package_directory: impl AsRef<Path>,
) -> Option<PathBuf> {
    let key = format!("RUNPOD_BINARY_{}_PATH", name.to_uppercase());
    if let Some(value) = values.get(&key).filter(|s| !s.is_empty()) {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Some(path);
        }
    }
    let path = package_directory.as_ref().join(name);
    path.is_file().then_some(path)
}
