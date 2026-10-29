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
