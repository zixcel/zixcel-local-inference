use super::manifest::{BundleManifest, MAX_FILES, invalid};
use crate::{LocalInferenceError, VerificationControl};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Read,
    path::Path,
};

pub(super) fn files(
    root: &Path,
    manifest: &BundleManifest,
    control: &VerificationControl,
) -> Result<(), LocalInferenceError> {
    control.check()?;
    let mut actual = BTreeSet::new();
    for entry in fs::read_dir(root).map_err(|_| invalid())? {
        control.check()?;
        let entry = entry.map_err(|_| invalid())?;
        let name = entry.file_name().into_string().map_err(|_| invalid())?;
        if !actual.insert(name) || actual.len() > MAX_FILES {
            return Err(invalid());
        }
    }
    let expected: BTreeSet<_> = manifest
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect();
    if actual != expected {
        return Err(invalid());
    }
    for item in &manifest.files {
        control.check()?;
        let path = root.join(&item.path);
        let meta = fs::symlink_metadata(&path).map_err(|_| invalid())?;
        if !meta.is_file() || meta.file_type().is_symlink() || meta.len() != item.bytes {
            return Err(invalid());
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(0x800 | 0x20000);
        }
        let mut file = options.open(&path).map_err(|_| invalid())?;
        let opened = file.metadata().map_err(|_| invalid())?;
        if !opened.is_file() || opened.len() != item.bytes {
            return Err(invalid());
        }
        let mut hash = Sha256::new();
        let mut buffer = [0_u8; 8192];
        let mut read = 0_u64;
        loop {
            control.check()?;
            let n = file.read(&mut buffer).map_err(|_| invalid())?;
            if n == 0 {
                break;
            }
            read = read.checked_add(n as u64).ok_or_else(invalid)?;
            if read > item.bytes {
                return Err(invalid());
            }
            hash.update(&buffer[..n]);
        }
        if read != item.bytes || super::manifest::hex(&hash.finalize()) != item.sha256 {
            return Err(LocalInferenceError::new("model-bundle-integrity-failed"));
        }
    }
    control.check()
}
