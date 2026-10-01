//! Stable admission configuration, deliberately excluding per-request inference input.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::LocalInferenceError;

pub const MAX_CONFIGURATION_BYTES: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelConfiguration {
    pub context_tokens: u32,
    pub batch_tokens: u32,
}

impl ModelConfiguration {
    /// Parses a bounded, closed configuration. Duplicate fields also reject.
    /// # Errors
    /// Unknown, oversized, missing or out-of-range fields reject.
    pub fn from_json(bytes: &[u8]) -> Result<Self, LocalInferenceError> {
        let value: Self = decode(bytes)?;
        value.validate()?;
        Ok(value)
    }

    pub(crate) fn validate(&self) -> Result<(), LocalInferenceError> {
        if !(1..=131_072).contains(&self.context_tokens)
            || !(1..=4096).contains(&self.batch_tokens)
            || self.batch_tokens > self.context_tokens
        {
            return Err(LocalInferenceError::new("invalid-configuration"));
        }
        Ok(())
    }

    /// Identity covers every admitted model configuration field.
    /// # Errors
    /// Invalid configuration rejects before identity creation.
    pub fn identity(&self) -> Result<String, LocalInferenceError> {
        self.validate()?;
        identity("model/configuration/1", self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeConfiguration {
    pub threads: u16,
    pub memory_bytes: u64,
}

impl RuntimeConfiguration {
    /// Parses stable CPU setup, not scheduling, prompts or decoding parameters.
    /// # Errors
    /// Unknown, oversized or out-of-range fields reject.
    pub fn from_json(bytes: &[u8]) -> Result<Self, LocalInferenceError> {
        let value: Self = decode(bytes)?;
        value.validate()?;
        Ok(value)
    }

    pub(crate) fn validate(&self) -> Result<(), LocalInferenceError> {
        if !(1..=256).contains(&self.threads)
            || !(16 * 1024 * 1024..=64 * 1024 * 1024 * 1024).contains(&self.memory_bytes)
        {
            return Err(LocalInferenceError::new("invalid-configuration"));
        }
        Ok(())
    }

    /// Identity excludes mutable observations such as available host RAM.
    /// # Errors
    /// Invalid configuration rejects before identity creation.
    pub fn identity(&self) -> Result<String, LocalInferenceError> {
        self.validate()?;
        identity("runtime/configuration/1", self)
    }
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, LocalInferenceError> {
    if bytes.len() > MAX_CONFIGURATION_BYTES {
        return Err(LocalInferenceError::new("invalid-configuration"));
    }
    serde_json::from_slice(bytes).map_err(|_| LocalInferenceError::new("invalid-configuration"))
}

pub(crate) fn identity(
    domain: &str,
    value: &impl Serialize,
) -> Result<String, LocalInferenceError> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| LocalInferenceError::new("invalid-configuration"))?;
    let mut hash = Sha256::new();
    hash.update(domain.as_bytes());
    hash.update([0]);
    hash.update(bytes);
    Ok(format!("{:x}", hash.finalize()))
}
