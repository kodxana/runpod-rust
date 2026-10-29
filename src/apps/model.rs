use crate::{Error, Result};
use std::{fmt, path::PathBuf, str::FromStr};

/// Hugging Face weights staged on platform hosts before worker startup.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Model {
    reference: String,
    owner: String,
    name: String,
    revision: Option<String>,
}
impl Model {
    /// Validates an owner/name reference with an optional revision suffix.
    ///
    /// # Errors
    /// Returns invalid input for empty or malformed model references.
    pub fn new(reference: impl Into<String>) -> Result<Self> {
        let reference = reference.into();
        if reference.is_empty() {
            return Err(Error::InvalidInput(
                "model reference must be a non-empty string".into(),
            ));
        }
        let (repo, revision) = reference
            .split_once(':')
            .map_or((reference.as_str(), None), |(repo, r)| {
                (repo, (!r.is_empty()).then_some(r))
            });
        let parts: Vec<_> = repo.split('/').collect();
        if parts.len() != 2
            || parts.iter().any(|p| {
                p.is_empty() || !p.chars().all(|c| c.is_alphanumeric() || "_.-".contains(c))
            })
        {
            return Err(Error::InvalidInput(format!(
                "invalid model reference '{reference}': expected 'owner/name' or 'owner/name:revision'"
            )));
        }
        Ok(Self {
            owner: parts[0].into(),
            name: parts[1].into(),
            revision: revision.map(str::to_owned),
            reference,
        })
    }
    /// Returns the exact API-facing reference.
    pub fn reference(&self) -> &str {
        &self.reference
    }
    /// Resolves the staged path, preferring MODEL_REVISION when set.
    pub fn path(&self) -> PathBuf {
        let revision = std::env::var("MODEL_REVISION")
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| self.revision.clone());
        let path = PathBuf::from("/runpod/model-store/huggingface")
            .join(&self.owner)
            .join(&self.name);
        revision.map_or_else(|| path.clone(), |r| path.join(r))
    }
    /// Returns the Hugging Face convention cache directory.
    pub fn hf_cache_path(&self) -> PathBuf {
        PathBuf::from("/runpod-volume/huggingface-cache/hub")
            .join(format!("models--{}--{}", self.owner, self.name))
    }
}
impl fmt::Display for Model {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reference)
    }
}
impl FromStr for Model {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        Self::new(value)
    }
}
