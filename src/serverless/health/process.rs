use super::HealthError;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt,
    path::PathBuf,
    process::{Output, Stdio},
    time::Duration,
};
use tokio::io::AsyncReadExt;

/// An executable and fixed arguments for a native health probe; no shell is invoked.
#[derive(Clone)]
pub struct ProbeCommand {
    /// Executable path or a name resolved through PATH.
    pub program: PathBuf,
    /// Arguments placed before probe-specific arguments.
    pub args: Vec<OsString>,
    /// Child-only environment overrides, without changing the parent environment.
    pub environment: BTreeMap<OsString, OsString>,
}
impl fmt::Debug for ProbeCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProbeCommand")
            .field("program", &self.program)
            .finish_non_exhaustive()
    }
}
impl ProbeCommand {
    /// Select an executable with no fixed arguments or environment overrides.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            environment: BTreeMap::new(),
        }
    }
    pub(super) async fn output(
        &self,
        args: &[&str],
        timeout: Duration,
        operation: &'static str,
    ) -> Result<Output, HealthError> {
        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(&self.args)
            .args(args)
            .envs(&self.environment)
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|source| HealthError::Io { operation, source })?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut out = child
            .stdout
            .take()
            .expect("stdout was configured as a pipe");
        let mut err = child
            .stderr
            .take()
            .expect("stderr was configured as a pipe");
        let result = tokio::time::timeout(timeout, async {
            let (_, _, status) = tokio::try_join!(
                out.read_to_end(&mut stdout),
                err.read_to_end(&mut stderr),
                child.wait()
            )?;
            Ok::<_, std::io::Error>(status)
        })
        .await;
        match result {
            Ok(Ok(status)) => Ok(Output {
                status,
                stdout,
                stderr,
            }),
            Ok(Err(source)) => {
                let _ = child.kill().await;
                Err(HealthError::Io { operation, source })
            }
            Err(_) => {
                let _ = child.kill().await;
                Err(HealthError::Timeout {
                    operation,
                    seconds: timeout.as_secs_f64(),
                })
            }
        }
    }
}
