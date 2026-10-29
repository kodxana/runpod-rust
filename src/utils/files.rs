use crate::Result;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// Resolves a model repository and optional revision into the platform cache path.
///
/// The `hugginface` spelling is retained for compatibility with the Python function.
#[must_use]
pub fn resolve_model_cache_path_from_hugginface_repository(
    repository: &str,
    path_template: Option<&str>,
) -> Option<String> {
    let (model, revision) = repository.rsplit_once(':').unwrap_or((repository, "main"));
    if model.is_empty() {
        return None;
    }
    Some(
        path_template
            .unwrap_or("/runpod/cache/{model}/{revision}")
            .replace("{model}", model)
            .replace("{revision}", revision),
    )
}

/// Probes nvidia-smi with a five-second deadline and returns whether CUDA is available.
pub async fn is_cuda_available() -> bool {
    let mut command = tokio::process::Command::new("nvidia-smi");
    command
        .kill_on_drop(true)
        .stderr(std::process::Stdio::null());
    match tokio::time::timeout(Duration::from_secs(5), command.output()).await {
        Ok(Ok(output)) => {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains("NVIDIA-SMI")
        }
        _ => false,
    }
}

/// Removes conventional job directories and explicitly named extra folders.
///
/// Directory-removal errors are ignored like Python's cleanup helper; output.zip errors
/// are returned. Relative extra paths are resolved against the supplied job directory.
///
/// # Errors
/// Returns an error if an existing output.zip cannot be removed.
pub fn clean(directory: impl AsRef<Path>, folders: &[PathBuf]) -> Result<()> {
    let directory = directory.as_ref();
    for name in ["input_objects", "output_objects", "job_files"] {
        let _ = std::fs::remove_dir_all(directory.join(name));
    }
    let archive = directory.join("output.zip");
    if archive.exists() {
        std::fs::remove_file(archive)?;
    }
    for folder in folders {
        let _ = std::fs::remove_dir_all(directory.join(folder));
    }
    Ok(())
}
