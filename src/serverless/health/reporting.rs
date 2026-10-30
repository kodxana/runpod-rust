use std::{collections::BTreeMap, io::Write, time::Duration};

/// Best-effort unhealthy heartbeat with a two-second total request deadline.
/// Missing credentials/URL/required worker ID and all HTTP failures are ignored.
pub async fn report_unhealthy(values: &BTreeMap<String, String>, check: &str, reason: &str) {
    let Some(url) = values
        .get("RUNPOD_WEBHOOK_PING")
        .filter(|s| !s.is_empty() && s.as_str() != "PING_NOT_SET")
    else {
        return;
    };
    let Some(key) = values.get("RUNPOD_AI_API_KEY").filter(|s| !s.is_empty()) else {
        return;
    };
    let worker = values.get("RUNPOD_POD_ID").map_or("", String::as_str);
    if url.contains("$RUNPOD_POD_ID") && worker.is_empty() {
        return;
    }
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    else {
        return;
    };
    let reason: String = reason.chars().take(256).collect();
    let _ = client
        .get(url.replace("$RUNPOD_POD_ID", worker))
        .header(reqwest::header::AUTHORIZATION, key)
        .query(&[
            ("status", "unhealthy"),
            ("check", check),
            ("reason", reason.as_str()),
            ("runpod_version", env!("CARGO_PKG_VERSION")),
        ])
        .send()
        .await;
}

/// Flush standard streams best-effort and immediately terminate every process thread.
/// This skips Rust destructors and C exit callbacks, matching Python's `os._exit`.
#[expect(
    unsafe_code,
    reason = "C _exit is the platform primitive for unconditional termination without exit callbacks"
)]
pub fn terminate_unhealthy(code: i32) -> ! {
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    // SAFETY: _exit accepts every C int and never returns. No pointer or reference crosses
    // the FFI boundary. Skipping destructors and exit handlers is the required behavior.
    unsafe { libc::_exit(code) }
}

pub(super) struct UnhealthyExit;
impl Drop for UnhealthyExit {
    fn drop(&mut self) {
        terminate_unhealthy(1);
    }
}
