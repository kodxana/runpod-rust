//! Profile credentials compatible with Python's `~/.runpod/config.toml`.
use crate::{Error, Result};
use std::{
    fmt,
    io::Write,
    path::{Path, PathBuf},
};
use toml_edit::{DocumentMut, Item, Table, value};

/// An API token whose debug representation does not reveal its value.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    /// Secret token for authenticated Runpod requests.
    pub api_key: String,
}
impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}
/// Controls whether setting credentials may replace an existing profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overwrite {
    /// Fail if the profile already exists.
    Deny,
    /// Replace the profile's credential fields.
    Allow,
}
/// A credential file with explicit ownership and atomic replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialStore {
    path: PathBuf,
}
impl CredentialStore {
    /// Opens a logical store without reading or creating its file.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    /// Locates the conventional user credential file.
    ///
    /// # Errors
    /// Returns invalid input if no user home directory can be resolved.
    pub fn user_default() -> Result<Self> {
        let home = if cfg!(windows) {
            std::env::var_os("USERPROFILE")
        } else {
            std::env::var_os("HOME")
        }
        .ok_or_else(|| Error::InvalidInput("home directory is not configured".into()))?;
        Ok(Self::new(
            PathBuf::from(home).join(".runpod").join("config.toml"),
        ))
    }
    /// Returns the backing file path.
    pub fn path(&self) -> &Path {
        &self.path
    }
    fn document(&self) -> Result<Option<DocumentMut>> {
        match std::fs::read_to_string(&self.path) {
            Ok(content) => content.parse().map(Some).map_err(|_| {
                Error::InvalidInput("~/.runpod/config.toml is not a valid TOML file.".into())
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
    /// Reads a profile, returning None for missing or malformed TOML/profiles.
    ///
    /// # Errors
    /// Returns filesystem errors other than not-found.
    pub fn get_credentials(&self, profile: &str) -> Result<Option<Credentials>> {
        let document = match self.document() {
            Ok(Some(document)) => document,
            Ok(None) | Err(Error::InvalidInput(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(document
            .get(profile)
            .and_then(Item::as_table_like)
            .and_then(|t| t.get("api_key"))
            .and_then(Item::as_str)
            .map(|key| Credentials {
                api_key: key.into(),
            }))
    }
    /// Checks that a profile has a string API token without contacting Runpod.
    ///
    /// # Errors
    /// Returns an explanatory configuration error for missing or malformed credentials.
    pub fn check_credentials(&self, profile: &str) -> Result<()> {
        let doc = self
            .document()?
            .ok_or_else(|| Error::InvalidInput("~/.runpod/config.toml does not exist.".into()))?;
        let entry = doc.get(profile).ok_or_else(|| {
            Error::InvalidInput(format!(
                "~/.runpod/config.toml is missing {profile} profile."
            ))
        })?;
        if entry
            .as_table_like()
            .and_then(|t| t.get("api_key"))
            .and_then(Item::as_str)
            .is_none()
        {
            return Err(Error::InvalidInput(format!(
                "~/.runpod/config.toml is missing api_key for {profile} profile."
            )));
        }
        Ok(())
    }
    /// Atomically writes a profile while preserving comments and unrelated profiles.
    ///
    /// # Errors
    /// Returns malformed configuration, duplicate-profile, or filesystem errors.
    pub fn set_credentials(
        &self,
        api_key: &str,
        profile: &str,
        overwrite: Overwrite,
    ) -> Result<()> {
        let mut doc = self.document()?.unwrap_or_default();
        if overwrite == Overwrite::Deny && doc.contains_key(profile) {
            return Err(Error::InvalidInput(
                "Profile already exists. Use set_credentials(overwrite=True) to update.".into(),
            ));
        }
        if let Some(Item::Table(table)) = doc.get_mut(profile) {
            table.clear();
            table.insert("api_key", value(api_key));
        } else {
            let mut table = Table::new();
            table.insert("api_key", value(api_key));
            doc[profile] = Item::Table(table);
        }
        let directory = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(directory)?;
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        temporary.write_all(doc.to_string().as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&self.path)
            .map_err(|e| Error::Io(e.error))?;
        Ok(())
    }
}
