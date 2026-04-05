use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::Add;
use std::str::FromStr;

/// A model identifier in the format "provider:model_name".
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelId(String);

impl ModelId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the provider portion (before the colon), if present.
    pub fn provider(&self) -> Option<&str> {
        self.0.split_once(':').map(|(p, _)| p)
    }

    /// Returns the model name portion (after the colon), or the full string.
    pub fn model_name(&self) -> &str {
        self.0.split_once(':').map_or(self.0.as_str(), |(_, m)| m)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for ModelId {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_owned()))
    }
}

impl From<&str> for ModelId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// Information about a model offered by a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: ModelId,
    pub display_name: String,
    pub context_length: u32,
    pub supports_tools: bool,
    pub supports_streaming: bool,
}

/// Token usage from a completion request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u32,
    pub output: u32,
    pub cache_read: u32,
}

impl TokenUsage {
    pub fn total(&self) -> u32 {
        self.input + self.output + self.cache_read
    }
}

impl Add for TokenUsage {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            input: self.input + rhs.input,
            output: self.output + rhs.output,
            cache_read: self.cache_read + rhs.cache_read,
        }
    }
}

/// Health status of a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProviderHealth {
    Healthy,
    Degraded { reason: String },
    Unavailable { reason: String },
}
