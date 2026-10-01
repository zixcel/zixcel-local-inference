use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use crate::LocalInferenceError;

pub(crate) const MAX_DOCUMENT_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) fn validate_absolute(path: &Path) -> Result<(), LocalInferenceError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(LocalInferenceError::new("path-invalid"));
    }
    Ok(())
}

pub(crate) fn prepare_directory(path: &Path) -> Result<PathBuf, LocalInferenceError> {
    validate_absolute(path)?;
    reject_symlink_components(path)?;
    fs::create_dir_all(path).map_err(|_| LocalInferenceError::new("state-io-failed"))?;
    existing_directory(path)
}

pub(crate) fn existing_directory(path: &Path) -> Result<PathBuf, LocalInferenceError> {
    validate_absolute(path)?;
    reject_symlink_components(path)?;
    let metadata =
        fs::symlink_metadata(path).map_err(|_| LocalInferenceError::new("directory-not-found"))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(LocalInferenceError::new("path-invalid"));
    }
    path.canonicalize()
        .map_err(|_| LocalInferenceError::new("path-invalid"))
}

pub(crate) fn open_registry_directory(path: &Path) -> Result<PathBuf, LocalInferenceError> {
    existing_directory(path).map_err(|error| {
        if error.code() == "directory-not-found" {
            LocalInferenceError::new("registry-missing")
        } else {
            error
        }
    })
}

fn reject_symlink_components(path: &Path) -> Result<(), LocalInferenceError> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(LocalInferenceError::new("path-invalid"));
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(LocalInferenceError::new("state-io-failed")),
        }
    }
    Ok(())
}

pub(crate) fn read_regular_file(path: &Path, maximum: u64) -> Result<Vec<u8>, LocalInferenceError> {
    validate_absolute(path)?;
    let metadata =
        fs::symlink_metadata(path).map_err(|_| LocalInferenceError::new("file-not-found"))?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > maximum
    {
        return Err(LocalInferenceError::new("file-invalid"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x800 | 0x20000); // O_NONBLOCK | O_NOFOLLOW
    }
    let file = options
        .open(path)
        .map_err(|_| LocalInferenceError::new("file-read-failed"))?;
    let opened = file
        .metadata()
        .map_err(|_| LocalInferenceError::new("file-read-failed"))?;
    if !opened.file_type().is_file() || opened.len() > maximum {
        return Err(LocalInferenceError::new("file-invalid"));
    }
    let mut bytes = Vec::new();
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| LocalInferenceError::new("file-read-failed"))?;
    if bytes.len() as u64 > maximum {
        return Err(LocalInferenceError::new("file-invalid"));
    }
    Ok(bytes)
}

pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<(), LocalInferenceError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| LocalInferenceError::new("state-conflict"))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| LocalInferenceError::new("state-io-failed"))
}

pub(crate) fn safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
}
