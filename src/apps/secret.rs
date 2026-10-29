use crate::{Error, Result};
use serde::{Serialize, Serializer};
use std::{fmt, str::FromStr};

/// A platform secret reference; the secret value never passes through this type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Secret {
    name: String,
}
impl Secret {
    /// Validates a secret name containing letters, digits, underscores, dots, or dashes.
    ///
    /// # Errors
    /// Returns invalid input for empty names or unsupported characters.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.is_empty() {
            return Err(Error::InvalidInput(
                "secret name must be a non-empty string".into(),
            ));
        }
        if !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
        {
            return Err(Error::InvalidInput(format!(
                "invalid secret name '{name}': letters, digits, underscores, dots, and dashes only"
            )));
        }
        Ok(Self { name })
    }
    /// Returns the platform secret's name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Renders the environment substitution syntax understood by the platform.
    pub fn reference(&self) -> String {
        format!("{{{{ RUNPOD_SECRET_{} }}}}", self.name)
    }
}
impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reference())
    }
}
impl FromStr for Secret {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        Self::new(value)
    }
}
impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.reference())
    }
}

/// Plain environment text or an explicit secret reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvValue {
    /// A literal environment value.
    Text(String),
    /// A secret substituted by the platform at worker startup.
    Secret(Secret),
}
impl From<String> for EnvValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}
impl From<&str> for EnvValue {
    fn from(value: &str) -> Self {
        Self::Text(value.into())
    }
}
impl From<Secret> for EnvValue {
    fn from(value: Secret) -> Self {
        Self::Secret(value)
    }
}
impl Serialize for EnvValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Text(v) => serializer.serialize_str(v),
            Self::Secret(v) => v.serialize(serializer),
        }
    }
}

/// Expands secret references while retaining environment insertion order.
pub fn render_env(env: &[(String, EnvValue)]) -> serde_json::Map<String, crate::Value> {
    env.iter()
        .map(|(key, value)| {
            (
                key.clone(),
                crate::Value::String(match value {
                    EnvValue::Text(value) => value.clone(),
                    EnvValue::Secret(secret) => secret.reference(),
                }),
            )
        })
        .collect()
}
/// Returns the names of all referenced secrets, retaining duplicates and order.
pub fn secret_names(env: &[(String, EnvValue)]) -> Vec<&str> {
    env.iter()
        .filter_map(|(_, v)| match v {
            EnvValue::Secret(s) => Some(s.name()),
            EnvValue::Text(_) => None,
        })
        .collect()
}
