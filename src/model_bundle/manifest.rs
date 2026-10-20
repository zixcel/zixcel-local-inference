use crate::LocalInferenceError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
pub(super) const MAX_FILES: usize = 64;
pub(super) const MAX_TOTAL: u64 = 16 * 1024 * 1024 * 1024;
pub(super) fn invalid() -> LocalInferenceError {
    LocalInferenceError::new("model-bundle-invalid")
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BundleFormat {
    OpenvinoIr,
    Onnx,
}
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BundleFile {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}
/// Flat, closed file set. Licence/revision are declarations, not approval evidence.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BundleManifest {
    pub schema: String,
    pub format: BundleFormat,
    pub source_revision: String,
    pub license: String,
    pub files: Vec<BundleFile>,
}
pub(super) fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl BundleManifest {
    pub(super) fn from_bytes(bytes: &[u8], expected: &str) -> Result<Self, LocalInferenceError> {
        if bytes.len() > 64 * 1024 || !digest(expected) || hex(&Sha256::digest(bytes)) != expected {
            return Err(invalid());
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<(), LocalInferenceError> {
        if self.schema != "zixcel://local-inference/model-bundle/v1"
            || !matches!(self.source_revision.len(), 40 | 64)
            || !self
                .source_revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.license.is_empty()
            || self.license.len() > 128
            || !self
                .license
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b" .-()+".contains(&b))
            || self.files.is_empty()
            || self.files.len() > MAX_FILES
        {
            return Err(invalid());
        }
        let mut total = 0_u64;
        let mut names = BTreeSet::new();
        let mut previous = "";
        for file in &self.files {
            if !crate::distribution::leaf(&file.path)
                || file.path.starts_with('.')
                || file.path.as_str() <= previous
                || !names.insert(file.path.to_ascii_lowercase())
                || !digest(&file.sha256)
            {
                return Err(invalid());
            }
            total = total.checked_add(file.bytes).ok_or_else(invalid)?;
            if total > MAX_TOTAL {
                return Err(invalid());
            }
            previous = &file.path;
        }
        if total == 0 {
            return Err(invalid());
        }
        Ok(())
    }
}

pub(super) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}
