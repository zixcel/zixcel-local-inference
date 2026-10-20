//! Read-only integrity of a complete model bundle. This is not runtime admission.
mod manifest;
mod verify;
use crate::{LocalInferenceError, VerificationControl};
pub use manifest::{BundleFile, BundleFormat, BundleManifest};
use std::path::{Path, PathBuf};

/// Verified observation of an exact bundle, without execution authority.
#[derive(Debug)]
pub struct VerifiedModelBundle {
    root: PathBuf,
    manifest: BundleManifest,
    manifest_sha256: String,
}
impl VerifiedModelBundle {
    /// Check all bytes against an externally selected manifest digest.
    /// No downloads, placement, signature trust or runtime activation occur.
    /// # Errors
    /// Rejects malformed manifests, unexpected paths and mismatched bytes.
    pub fn verify(
        root: &Path,
        document: &[u8],
        expected_sha256: &str,
        control: &VerificationControl,
    ) -> Result<Self, LocalInferenceError> {
        let manifest = BundleManifest::from_bytes(document, expected_sha256)?;
        let root = crate::fsutil::existing_directory(root)?;
        verify::files(&root, &manifest, control)?;
        Ok(Self {
            root,
            manifest,
            manifest_sha256: expected_sha256.into(),
        })
    }
    /// Recheck the same observed identity before backend use.
    /// # Errors
    /// Returns the original verification failure; never repairs files.
    pub fn revalidate(&self, control: &VerificationControl) -> Result<(), LocalInferenceError> {
        crate::fsutil::existing_directory(&self.root)?;
        verify::files(&self.root, &self.manifest, control)
    }
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
    #[must_use]
    pub const fn manifest(&self) -> &BundleManifest {
        &self.manifest
    }
    #[must_use]
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }
}
