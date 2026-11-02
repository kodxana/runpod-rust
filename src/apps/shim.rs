//! POSIX launchers installed into custom remote images; no local Python bridge is used.
use super::ResourceKind;

/// Constructs the remote container command that installs and starts the runtime.
#[must_use]
pub fn runtime_launcher(kind: ResourceKind) -> String {
    let module = match kind {
        ResourceKind::Api | ResourceKind::Queue => "runpod_sdk_runtime.bootstrap",
        ResourceKind::Task => "runpod_sdk_runtime.task.runner",
    };
    // Keep the body single-quote-free: Runpod shell-lexes dockerArgs.
    let script = concat!(
        "PY=\"\"; for c in python3 python /usr/local/bin/python3 /usr/bin/python3 /opt/conda/bin/python /opt/venv/bin/python /venv/bin/python /root/.venv/bin/python /app/.venv/bin/python /usr/local/bin/python; do ",
        "if command -v \"$c\" >/dev/null 2>&1; then PY=\"$c\"; break; fi; done; ",
        "if [ -z \"$PY\" ]; then echo \"[shim] FATAL: no python interpreter found in this image. \"\"custom images must include python3.\" >&2; exit 1; fi; ",
        "export SETUPTOOLS_SCM_PRETEND_VERSION_FOR_RUNPOD=\"${SETUPTOOLS_SCM_PRETEND_VERSION_FOR_RUNPOD:-0.0.0.dev0}\"; ",
        "RUNTIME_SPEC=\"${RUNPOD_RUNTIME_PACKAGE_SPEC:-runpod-sdk-runtime}\"; ",
        "if [ -n \"${RUNPOD_RUNTIME_PACKAGE_SPEC:-}\" ] || ! \"$PY\" -c \"import runpod_sdk_runtime\" >/dev/null 2>&1; then ",
        "\"$PY\" -m pip install -q --upgrade \"$RUNTIME_SPEC\" || exit 1; fi; ",
        "SDK_SPEC=\"${RUNPOD_PACKAGE_SPEC:-runpod}\"; ",
        "if [ -n \"${RUNPOD_PACKAGE_SPEC:-}\" ] || ! \"$PY\" -c \"import runpod\" >/dev/null 2>&1; then ",
        "\"$PY\" -m pip install -q --upgrade \"$SDK_SPEC\" || exit 1; fi; exec \"$PY\" -m "
    );
    format!("sh -c '{script}{module}'")
}

/// Constructs a POSIX container command that starts an archived native worker.
///
/// Requires the tools and image compatibility documented by
/// [`super::deploy::deploy_native_app`]. Paths come from the same environment settings as
/// `BootstrapConfig`. A prebuilt directory or warm marker takes precedence over a tarball.
/// Cold startup extracts only the fixed executable to a separate bootstrap filename, so
/// the running binary is not overwritten when Rust subsequently extracts the full archive.
/// Failures before the executable starts are stderr/exit diagnostics; structured worker
/// failure reporting becomes available inside `start_deployed`.
#[must_use]
pub fn native_runtime_launcher() -> String {
    // Single-quote-free body: the platform shell-lexes dockerArgs before launching sh.
    let script = concat!(
        "APP=\"${RUNPOD_APP_DIR-/app}\"; [ -n \"$APP\" ] || APP=.; ",
        "PREBUILT=\"${RUNPOD_PREBUILT_APP_DIR-}\"; ",
        "WAIT=\"${RUNPOD_ARTIFACT_WAIT-300}\"; ",
        "if ! [ \"$WAIT\" -eq \"$WAIT\" ] 2>/dev/null; then echo \"[native] RUNPOD_ARTIFACT_WAIT must be an integer\" >&2; exit 1; fi; ",
        "if [ -n \"$PREBUILT\" ] && [ -d \"$PREBUILT\" ]; then WORKER=\"$PREBUILT/runpod-native/worker\"; ",
        "elif [ -f \"$APP/.rp-unpacked\" ]; then WORKER=\"$APP/runpod-native/worker\"; ",
        "else ARTIFACT=\"${FLASH_BUILD_ARTIFACT_PATH-/root/.runpod/artifact.tar.gz}\"; ELAPSED=0; ",
        "while [ ! -f \"$ARTIFACT\" ]; do ",
        "if [ \"$ELAPSED\" -ge \"$WAIT\" ]; then echo \"[native] artifact not found at $ARTIFACT after ${ELAPSED}s\" >&2; exit 1; fi; ",
        "sleep 2 || exit 1; ELAPSED=$((ELAPSED + 2)); done; ",
        "mkdir -p \"$APP/runpod-native\" || exit 1; WORKER=\"$APP/runpod-native/bootstrap\"; ",
        "tar -xOzf - runpod-native/worker < \"$ARTIFACT\" > \"$WORKER\" || exit 1; ",
        "chmod 700 \"$WORKER\" || exit 1; fi; ",
        "if [ ! -f \"$WORKER\" ] || [ ! -x \"$WORKER\" ]; then echo \"[native] worker is missing or not executable: $WORKER\" >&2; exit 1; fi; ",
        "exec \"$WORKER\" \"$@\""
    );
    format!("sh -c '{script}'")
}
