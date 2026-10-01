use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::fsutil::{
    MAX_DOCUMENT_BYTES, existing_directory, prepare_directory, read_regular_file, safe_identifier,
    validate_absolute, write_new,
};
use crate::{
    AcquisitionPlan, CatalogStore, INSTALLATION_SCHEMA, InstallationStatus, InstalledModel,
    LocalInferenceError, Selection,
};

const COPY_BUFFER_BYTES: usize = 1024 * 1024;

/// Imports Crowsi-delivered bytes after checking signed catalog evidence.
///
/// # Errors
/// Returns an error for unsafe paths, incompatible selections, digest mismatch, conflicts, or I/O.
pub fn install_from_file(
    store: &CatalogStore,
    model_id: &str,
    model_root: &Path,
    selection: &Selection,
    delivered_file: &Path,
) -> Result<InstallationStatus, LocalInferenceError> {
    let plan = store.acquisition_plan(model_id, model_root, selection)?;
    let canonical_root = prepare_directory(model_root)?;
    let source_metadata = fs::symlink_metadata(delivered_file)
        .map_err(|_| LocalInferenceError::new("artifact-delivery-not-found"))?;
    if !source_metadata.file_type().is_file() || source_metadata.file_type().is_symlink() {
        return Err(LocalInferenceError::new("artifact-delivery-invalid"));
    }
    if source_metadata.len() != plan.expected_bytes {
        return Err(LocalInferenceError::new("artifact-size-mismatch"));
    }

    let model_directory = canonical_root.join(&plan.model_id);
    fs::create_dir_all(&model_directory)
        .map_err(|_| LocalInferenceError::new("installation-io-failed"))?;
    ensure_plain_directory(&model_directory)?;
    let release_directory = model_directory.join(&plan.release_id);
    fs::create_dir_all(&release_directory)
        .map_err(|_| LocalInferenceError::new("installation-io-failed"))?;
    ensure_plain_directory(&release_directory)?;
    let release_directory = release_directory
        .canonicalize()
        .map_err(|_| LocalInferenceError::new("installation-io-failed"))?;
    if !release_directory.starts_with(&canonical_root) {
        return Err(LocalInferenceError::new("installation-root-invalid"));
    }
    let manifest_path = release_directory.join("installation.json");
    if manifest_path.exists() {
        let statuses = installation_statuses(store, &canonical_root, &plan.model_id)?;
        let existing = statuses
            .into_iter()
            .find(|status| {
                status.release_id == plan.release_id && status.artifact_id == plan.artifact_id
            })
            .ok_or_else(|| LocalInferenceError::new("installation-state-conflict"))?;
        if existing.artifact_sha256 != plan.expected_sha256
            || existing.artifact_bytes != plan.expected_bytes
            || existing.format != plan.format
        {
            return Err(LocalInferenceError::new("installation-state-conflict"));
        }
        return Ok(existing);
    }
    let artifact_path = release_directory.join(
        plan.destination
            .file_name()
            .ok_or_else(|| LocalInferenceError::new("installation-path-invalid"))?,
    );
    if artifact_path.exists() {
        let (digest, bytes) = digest_file(&artifact_path)?;
        if digest != plan.expected_sha256 || bytes != plan.expected_bytes {
            return Err(LocalInferenceError::new("installation-state-conflict"));
        }
        let status = status_from_plan(store, plan, artifact_path, digest, bytes)?;
        persist_status(store, &canonical_root, &status)?;
        return Ok(status);
    }
    let temporary_path = release_directory.join(format!(
        ".{}.partial-{}",
        plan.artifact_id,
        std::process::id()
    ));
    let (digest, bytes) = copy_and_digest(delivered_file, &temporary_path)?;
    if digest != plan.expected_sha256 || bytes != plan.expected_bytes {
        let _ = fs::remove_file(&temporary_path);
        return Err(LocalInferenceError::new("artifact-digest-mismatch"));
    }
    fs::rename(&temporary_path, &artifact_path)
        .map_err(|_| LocalInferenceError::new("installation-io-failed"))?;
    let status = status_from_plan(store, plan, artifact_path, digest, bytes)?;
    if let Err(error) = persist_status(store, &canonical_root, &status) {
        let _ = fs::remove_file(&status.artifact_path);
        return Err(error);
    }
    Ok(status)
}

/// Validates all installed releases for one model against their signed catalog generations.
///
/// # Errors
/// Returns an error for unsafe state, missing evidence, mutation, or I/O.
pub fn installation_statuses(
    store: &CatalogStore,
    model_root: &Path,
    model_id: &str,
) -> Result<Vec<InstallationStatus>, LocalInferenceError> {
    if !safe_identifier(model_id) {
        return Err(LocalInferenceError::new("model-id-invalid"));
    }
    let canonical_root = existing_directory(model_root)?;
    let model_directory = canonical_root.join(model_id);
    if !model_directory.exists() {
        return Ok(Vec::new());
    }
    let metadata = fs::symlink_metadata(&model_directory)
        .map_err(|_| LocalInferenceError::new("installation-state-invalid"))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(LocalInferenceError::new("installation-state-invalid"));
    }
    let mut statuses = Vec::new();
    for entry in fs::read_dir(model_directory)
        .map_err(|_| LocalInferenceError::new("installation-io-failed"))?
    {
        let entry = entry.map_err(|_| LocalInferenceError::new("installation-io-failed"))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("installation.json");
        if !manifest_path.exists() {
            return Err(LocalInferenceError::new("installation-manifest-missing"));
        }
        let bytes = read_regular_file(&manifest_path, MAX_DOCUMENT_BYTES)?;
        let status: InstallationStatus = serde_json::from_slice(&bytes)
            .map_err(|_| LocalInferenceError::new("installation-manifest-invalid"))?;
        verify_status(store, &canonical_root, &status)?;
        statuses.push(status);
    }
    statuses.sort_by(|left, right| left.release_id.cmp(&right.release_id));
    Ok(statuses)
}

/// Lists every verified installed model beneath one model root.
///
/// # Errors
/// Returns an error when the model root or any installation cannot be verified.
pub fn installed_models(
    store: &CatalogStore,
    model_root: &Path,
) -> Result<Vec<InstalledModel>, LocalInferenceError> {
    let canonical_root = existing_directory(model_root)?;
    let mut installed = Vec::new();
    for entry in fs::read_dir(&canonical_root)
        .map_err(|_| LocalInferenceError::new("installation-io-failed"))?
    {
        let entry = entry.map_err(|_| LocalInferenceError::new("installation-io-failed"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.path().is_dir() || !safe_identifier(&name) {
            continue;
        }
        for status in installation_statuses(store, &canonical_root, &name)? {
            installed.push(InstalledModel {
                model_id: status.model_id,
                release_id: status.release_id,
                state: status.state,
                artifact_path: status.artifact_path,
            });
        }
    }
    installed.sort_by(|left, right| {
        left.model_id
            .cmp(&right.model_id)
            .then_with(|| left.release_id.cmp(&right.release_id))
    });
    Ok(installed)
}

/// Removes exactly one confirmed model release directory.
///
/// # Errors
/// Returns an error for invalid confirmation, unsafe paths, missing releases, or I/O.
pub fn remove_installation(
    model_root: &Path,
    model_id: &str,
    release_id: &str,
    confirmation: &str,
) -> Result<(), LocalInferenceError> {
    if !safe_identifier(model_id)
        || !safe_identifier(release_id)
        || confirmation != format!("{model_id}@{release_id}")
    {
        return Err(LocalInferenceError::new("removal-confirmation-invalid"));
    }
    let canonical_root = existing_directory(model_root)?;
    let model_directory = canonical_root.join(model_id);
    ensure_plain_directory(&model_directory)
        .map_err(|_| LocalInferenceError::new("installation-not-found"))?;
    let release_directory = model_directory.join(release_id);
    ensure_plain_directory(&release_directory)
        .map_err(|_| LocalInferenceError::new("installation-not-found"))?;
    let canonical_release = release_directory
        .canonicalize()
        .map_err(|_| LocalInferenceError::new("installation-not-found"))?;
    if !canonical_release.starts_with(&canonical_root) {
        return Err(LocalInferenceError::new("installation-root-invalid"));
    }
    fs::remove_dir_all(&canonical_release)
        .map_err(|_| LocalInferenceError::new("installation-io-failed"))?;
    if fs::read_dir(&model_directory)
        .map_err(|_| LocalInferenceError::new("installation-io-failed"))?
        .next()
        .is_none()
    {
        fs::remove_dir(model_directory)
            .map_err(|_| LocalInferenceError::new("installation-io-failed"))?;
    }
    Ok(())
}

fn status_from_plan(
    store: &CatalogStore,
    plan: AcquisitionPlan,
    artifact_path: PathBuf,
    artifact_sha256: String,
    artifact_bytes: u64,
) -> Result<InstallationStatus, LocalInferenceError> {
    let (model, _) = store.installation_evidence(
        &plan.source_id,
        &plan.catalog_id,
        plan.catalog_sequence,
        &plan.model_id,
        &plan.release_id,
        &plan.artifact_id,
    )?;
    Ok(InstallationStatus {
        schema: INSTALLATION_SCHEMA.to_owned(),
        source_id: plan.source_id,
        catalog_id: plan.catalog_id,
        catalog_sequence: plan.catalog_sequence,
        model_id: plan.model_id,
        display_name: model.display_name,
        release_id: plan.release_id,
        artifact_id: plan.artifact_id,
        artifact_kind: plan.artifact_kind,
        format: plan.format,
        artifact_path,
        artifact_sha256,
        artifact_bytes,
        runtime_engines: plan.runtime_engines,
        state: "artifact-ready".to_owned(),
        inference_ready: false,
    })
}

fn persist_status(
    store: &CatalogStore,
    canonical_root: &Path,
    status: &InstallationStatus,
) -> Result<(), LocalInferenceError> {
    let manifest = serde_json::to_vec_pretty(&status)
        .map_err(|_| LocalInferenceError::new("installation-manifest-invalid"))?;
    let manifest_path = canonical_root
        .join(&status.model_id)
        .join(&status.release_id)
        .join("installation.json");
    write_new(&manifest_path, &manifest)?;
    if let Err(error) = verify_status(store, canonical_root, status) {
        let _ = fs::remove_file(manifest_path);
        return Err(error);
    }
    Ok(())
}

fn ensure_plain_directory(path: &Path) -> Result<(), LocalInferenceError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| LocalInferenceError::new("installation-state-invalid"))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(LocalInferenceError::new("installation-state-invalid"));
    }
    Ok(())
}

fn copy_and_digest(
    source: &Path,
    destination: &Path,
) -> Result<(String, u64), LocalInferenceError> {
    validate_absolute(source)?;
    let mut input =
        File::open(source).map_err(|_| LocalInferenceError::new("artifact-delivery-not-found"))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| LocalInferenceError::new("installation-state-conflict"))?;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|_| LocalInferenceError::new("artifact-read-failed"))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        output
            .write_all(&buffer[..count])
            .map_err(|_| LocalInferenceError::new("installation-io-failed"))?;
        total = total
            .checked_add(
                u64::try_from(count)
                    .map_err(|_| LocalInferenceError::new("artifact-size-invalid"))?,
            )
            .ok_or_else(|| LocalInferenceError::new("artifact-size-invalid"))?;
    }
    output
        .sync_all()
        .map_err(|_| LocalInferenceError::new("installation-io-failed"))?;
    Ok((format!("{:x}", hasher.finalize()), total))
}

fn verify_status(
    store: &CatalogStore,
    root: &Path,
    status: &InstallationStatus,
) -> Result<(), LocalInferenceError> {
    if status.schema != INSTALLATION_SCHEMA
        || !safe_identifier(&status.model_id)
        || !safe_identifier(&status.release_id)
        || !safe_identifier(&status.artifact_id)
        || status.state != "artifact-ready"
        || status.inference_ready
        || status.artifact_bytes == 0
        || status.artifact_sha256.len() != 64
    {
        return Err(LocalInferenceError::new("installation-manifest-invalid"));
    }
    let (model, artifact) = store.installation_evidence(
        &status.source_id,
        &status.catalog_id,
        status.catalog_sequence,
        &status.model_id,
        &status.release_id,
        &status.artifact_id,
    )?;
    if status.display_name != model.display_name
        || status.artifact_kind != artifact.kind
        || status.format != artifact.format
        || status.artifact_sha256 != artifact.integrity.sha256
        || status.artifact_bytes != artifact.integrity.bytes
        || status.runtime_engines != artifact.runtime_engines
        || status.artifact_path.file_name() != Some(artifact.filename.as_ref())
    {
        return Err(LocalInferenceError::new("installation-evidence-mismatch"));
    }
    let expected_parent = root.join(&status.model_id).join(&status.release_id);
    if status.artifact_path.parent() != Some(expected_parent.as_path()) {
        return Err(LocalInferenceError::new("installation-manifest-invalid"));
    }
    let metadata = fs::symlink_metadata(&status.artifact_path)
        .map_err(|_| LocalInferenceError::new("artifact-missing"))?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() != status.artifact_bytes
    {
        return Err(LocalInferenceError::new("artifact-size-mismatch"));
    }
    let (digest, bytes) = digest_file(&status.artifact_path)?;
    if digest != status.artifact_sha256 || bytes != status.artifact_bytes {
        return Err(LocalInferenceError::new("artifact-digest-mismatch"));
    }
    Ok(())
}

fn digest_file(path: &Path) -> Result<(String, u64), LocalInferenceError> {
    let mut file = File::open(path).map_err(|_| LocalInferenceError::new("artifact-missing"))?;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| LocalInferenceError::new("artifact-read-failed"))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        total = total
            .checked_add(
                u64::try_from(count)
                    .map_err(|_| LocalInferenceError::new("artifact-size-invalid"))?,
            )
            .ok_or_else(|| LocalInferenceError::new("artifact-size-invalid"))?;
    }
    Ok((format!("{:x}", hasher.finalize()), total))
}
