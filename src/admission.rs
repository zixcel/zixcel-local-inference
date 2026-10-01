//! Exact definitions only. No model selection, process, route, Grant or inference.
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zixcel_revision::{
    CommitIntent, CommitOutcome, CommitReceipt, CommitStore, Failure, RedbBackend, RevisionRef,
};

use crate::configuration::identity;
use crate::fsutil::{open_registry_directory, prepare_directory, safe_identifier};
use crate::{
    AdmittedDistribution, LocalInferenceError, ModelConfiguration, RuntimeConfiguration,
    RuntimeDistribution, VerificationControl,
};

const DOMAIN: &str = "zixcel/model/runtime/admission/1";
#[path = "cpu_selection.rs"]
mod cpu_selection;
#[path = "process_requests.rs"]
mod process_requests;
pub use cpu_selection::{CpuDerivation, CpuPreparation, CpuSelectionObservation};
pub const MAX_ADMITTED_ARTIFACTS: usize = 64;
pub const MAX_ADMITTED_RUNTIMES: usize = 128;
pub const MAX_REGISTRY_PAYLOAD_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactFormat {
    GgufV3,
    StaticElf64,
    DynamicElf64,
}

/// Explicit owner assertion, preserved verbatim, not a claim of signature verification.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactProvenance {
    pub publisher: String,
    pub license: String,
    pub source: String,
    pub revision: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactAdmission {
    pub sha256: String,
    pub bytes: u64,
    pub format: ArtifactFormat,
    pub provenance: ArtifactProvenance,
}

impl ArtifactAdmission {
    pub(crate) fn validate(&self) -> Result<(), LocalInferenceError> {
        if !digest(&self.sha256) || self.bytes == 0 || self.bytes > 8 * 1024 * 1024 * 1024 {
            return Err(LocalInferenceError::new("unsupported-artifact"));
        }
        for value in [
            &self.provenance.publisher,
            &self.provenance.license,
            &self.provenance.source,
            &self.provenance.revision,
        ] {
            if value.trim().is_empty() || value.len() > 2048 || value.chars().any(char::is_control)
            {
                return Err(LocalInferenceError::new("artifact-provenance-invalid"));
            }
        }
        if self.provenance.source.starts_with('/') || self.provenance.source.starts_with("file:") {
            return Err(LocalInferenceError::new("artifact-provenance-invalid"));
        }
        if !safe_identifier(&self.provenance.publisher)
            || !safe_identifier(&self.provenance.revision)
            || self.provenance.source.contains(['?', '#', '@', '\\'])
            || !(self.provenance.source.starts_with("https://")
                || self.provenance.source.starts_with("urn:"))
        {
            return Err(LocalInferenceError::new("artifact-provenance-invalid"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmittedArtifact {
    pub artifact_ref: String,
    pub definition: ArtifactAdmission,
    /// Architecture obtained from bytes, never inferred from a filename.
    pub architecture: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeAdmission {
    pub model_artifact_ref: String,
    pub implementation_ref: String,
    pub model_configuration: ModelConfiguration,
    pub runtime_configuration: RuntimeConfiguration,
    /// Explicit implementation compatibility assertion; not runtime observation.
    pub engine: String,
    pub engine_revision: String,
    pub model_architecture: String,
}

impl RuntimeAdmission {
    fn validate(&self) -> Result<(), LocalInferenceError> {
        self.model_configuration.validate()?;
        self.runtime_configuration.validate()?;
        if !digest(&self.model_artifact_ref)
            || !digest(&self.implementation_ref)
            || !safe_identifier(&self.engine)
            || !safe_identifier(&self.engine_revision)
            || !safe_identifier(&self.model_architecture)
        {
            return Err(LocalInferenceError::new("invalid-configuration"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmittedRuntime {
    pub runtime_ref: String,
    pub model_config_ref: String,
    pub runtime_config_ref: String,
    pub definition: RuntimeAdmission,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Image {
    artifacts: Vec<AdmittedArtifact>,
    distributions: Vec<AdmittedDistribution>,
    runtimes: Vec<AdmittedRuntime>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrySnapshot {
    pub revision: RevisionRef,
    pub artifacts: Vec<AdmittedArtifact>,
    pub distributions: Vec<AdmittedDistribution>,
    pub runtimes: Vec<AdmittedRuntime>,
}

#[derive(Clone, Debug)]
pub struct RuntimeRegistry {
    root: PathBuf,
}

impl RuntimeRegistry {
    /// Owner-only launch preparation: exact admitted definitions/bytes, not host readiness.
    /// Immutable definitions are captured under the registry lock; slow byte verification
    /// does not retain a Graph transaction or block process inspection/cancellation.
    pub(crate) fn verify_execution_artifacts(
        &self,
        reference: &str,
        control: &VerificationControl,
    ) -> Result<(), LocalInferenceError> {
        let image = {
            let _lock = self.lock(false, control)?;
            read_image(&self.store()?)?.1
        };
        let runtime = image
            .runtimes
            .iter()
            .find(|r| r.runtime_ref == reference)
            .ok_or_else(|| err("runtime-not-found"))?;
        self.verify_runtime_definition(&image, &runtime.definition, control)
    }
    /// Explicit physical recovery of an existing Graph database after an unclean exit.
    /// Does not initialize missing storage, alter definitions or repair model bytes.
    /// # Errors
    /// Missing/corrupt owner state and recovery failures remain typed errors.
    pub fn recover(root: &Path) -> Result<Self, LocalInferenceError> {
        let root = open_registry_directory(root)?;
        open_registry_directory(&root.join("objects"))?;
        let registry = Self { root };
        {
            let _lock = registry.lock(true, &VerificationControl::new(Duration::from_secs(2)))?;
            let backend = RedbBackend::recover_existing(registry.root.join("admission.redb"))
                .map_err(|error| graph_error(&error))?;
            read_image(&CommitStore::new(backend))?;
        }
        Ok(registry)
    }

    /// Opens existing admission storage without repair, creation or default selection.
    /// # Errors
    /// Missing/corrupt storage remains explicit.
    pub fn open_existing(root: &Path) -> Result<Self, LocalInferenceError> {
        let root = open_registry_directory(root)?;
        open_registry_directory(&root.join("objects"))?;
        let registry = Self { root };
        registry.snapshot()?;
        Ok(registry)
    }

    /// Explicit provisioning only. Existing storage must be opened, never overwritten.
    /// # Errors
    /// Existing, invalid or unavailable storage rejects.
    pub fn provision(root: &Path) -> Result<Self, LocalInferenceError> {
        if root.try_exists().map_err(|_| err("registry-unavailable"))? {
            return Err(err("admission-conflict"));
        }
        let root = prepare_directory(root)?;
        prepare_directory(&root.join("objects"))?;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join("admission.lock"))
            .and_then(|file| file.sync_all())
            .map_err(|_| err("registry-unavailable"))?;
        let backend = RedbBackend::create(root.join("admission.redb"))
            .map_err(|error| graph_error(&error))?;
        drop(backend);
        File::open(&root)
            .and_then(|file| file.sync_all())
            .map_err(|_| err("registry-unavailable"))?;
        Self::open_existing(&root)
    }

    /// Read-only projection of already admitted definitions, never process status.
    /// # Errors
    /// Typed absence, corruption, capacity or bounded lock contention.
    pub fn snapshot(&self) -> Result<RegistrySnapshot, LocalInferenceError> {
        let _lock = self.lock(false, &VerificationControl::new(Duration::from_secs(2)))?;
        let store = self.store()?;
        let (revision, image) = read_image(&store)?;
        Ok(RegistrySnapshot {
            revision,
            artifacts: image.artifacts,
            distributions: image.distributions,
            runtimes: image.runtimes,
        })
    }

    /// Resolves an exact identity, without replacement or download if bytes disappeared.
    /// # Errors
    /// Unknown identity remains `runtime-not-found`.
    pub fn runtime(&self, reference: &str) -> Result<AdmittedRuntime, LocalInferenceError> {
        self.snapshot()?
            .runtimes
            .into_iter()
            .find(|v| v.runtime_ref == reference)
            .ok_or_else(|| err("runtime-not-found"))
    }

    /// Resolves existing artifact metadata without rematerializing bytes.
    /// # Errors
    /// Unknown identity remains `artifact-not-found`.
    pub fn artifact(&self, reference: &str) -> Result<AdmittedArtifact, LocalInferenceError> {
        self.snapshot()?
            .artifacts
            .into_iter()
            .find(|v| v.artifact_ref == reference)
            .ok_or_else(|| err("artifact-not-found"))
    }

    /// Exact immutable layout admission, including all owned dependency bytes.
    /// # Errors
    /// Unknown artifacts, unsafe/unclosed layouts, stale CAS and capacity reject.
    pub fn admit_distribution(
        &self,
        definition: RuntimeDistribution,
        expected: RevisionRef,
    ) -> Result<(AdmittedDistribution, CommitReceipt), LocalInferenceError> {
        let control = VerificationControl::new(Duration::from_secs(30));
        let _lock = self.lock(true, &control)?;
        let store = self.store()?;
        let (revision, mut image) = read_image(&store)?;
        definition.validate(&image.artifacts)?;
        let reference = identity("runtime/distribution/1", &definition)?;
        if let Some(existing) = image
            .distributions
            .iter()
            .find(|d| d.distribution_ref == reference)
        {
            return Ok((existing.clone(), receipt(&store, &reference)?));
        }
        if revision != expected {
            return Err(err("admission-conflict"));
        }
        if image.distributions.len() >= 32 {
            return Err(err("capacity-exceeded"));
        }
        self.verify_distribution_definition(&image, &definition, &control)?;
        let value = AdmittedDistribution {
            distribution_ref: reference,
            definition,
        };
        image.distributions.push(value.clone());
        image
            .distributions
            .sort_by(|a, b| a.distribution_ref.cmp(&b.distribution_ref));
        let prepared = prepare(&image, &value.distribution_ref, expected)?;
        Ok((value, commit(&store, &prepared)?))
    }

    /// Rechecks bytes and dependency closure; does not deploy a loader layout.
    /// # Errors
    /// Missing/corrupt bytes and changed closure reject, without any mutation.
    pub fn verify_distribution(&self, reference: &str) -> Result<(), LocalInferenceError> {
        let control = VerificationControl::new(Duration::from_secs(30));
        let _lock = self.lock(true, &control)?;
        let (_, image) = read_image(&self.store()?)?;
        let value = image
            .distributions
            .iter()
            .find(|d| d.distribution_ref == reference)
            .ok_or_else(|| err("distribution-not-found"))?;
        self.verify_distribution_definition(&image, &value.definition, &control)
    }

    fn verify_distribution_definition(
        &self,
        image: &Image,
        definition: &RuntimeDistribution,
        control: &VerificationControl,
    ) -> Result<(), LocalInferenceError> {
        definition.validate(&image.artifacts)?;
        let mut metadata = std::collections::BTreeMap::new();
        for file in &definition.files {
            let artifact = image
                .artifacts
                .iter()
                .find(|a| a.artifact_ref == file.artifact_ref)
                .ok_or_else(|| err("artifact-not-found"))?;
            let verified = crate::verification::verify(
                &self.object_path(&artifact.definition.sha256),
                &artifact.definition,
                control,
            )?;
            metadata.insert(
                file.path.clone(),
                verified.dynamic.ok_or_else(|| err("unsupported-runtime"))?,
            );
        }
        definition.verify_closure(&metadata)
    }

    /// Checks exact admitted bytes. No write, recovery or process is performed.
    /// # Errors
    /// Missing, corrupt or timed-out artifacts remain distinguishable.
    pub fn verify_artifact(&self, reference: &str) -> Result<(), LocalInferenceError> {
        let control = VerificationControl::new(Duration::from_secs(30));
        let _lock = self.lock(true, &control)?;
        let (_, image) = read_image(&self.store()?)?;
        let artifact = image
            .artifacts
            .iter()
            .find(|value| value.artifact_ref == reference)
            .ok_or_else(|| err("artifact-not-found"))?;
        crate::verification::verify(
            &self.object_path(&artifact.definition.sha256),
            &artifact.definition,
            &control,
        )
        .map(|_| ())
        .map_err(|error| match error.code() {
            "artifact-not-found" => err("artifact-unavailable"),
            "digest-mismatch" | "unsupported-artifact" => err("artifact-corrupt"),
            _ => error,
        })
    }

    /// Rechecks the admitted CPU/static-backend envelope and both exact artifacts.
    /// This is a current observation, not a new admission or an inference probe.
    /// # Errors
    /// Missing bytes, corruption, host mismatch and timeout leave identity unchanged.
    pub fn verify_runtime(&self, reference: &str) -> Result<(), LocalInferenceError> {
        let control = VerificationControl::new(Duration::from_secs(30));
        let _lock = self.lock(true, &control)?;
        let (_, image) = read_image(&self.store()?)?;
        let runtime = image
            .runtimes
            .iter()
            .find(|value| value.runtime_ref == reference)
            .ok_or_else(|| err("runtime-not-found"))?;
        self.verify_runtime_definition(&image, &runtime.definition, &control)
            .map_err(|error| {
                if error.code() == "unsupported-runtime" {
                    err("runtime-unavailable")
                } else {
                    error
                }
            })?;
        if image
            .distributions
            .iter()
            .any(|d| d.distribution_ref == runtime.definition.implementation_ref)
        {
            // Admission proves requirements and owned bytes, not the host loader's
            // current symbol/CPU/backend availability. M4-B owns that observation.
            return Err(err("runtime-host-verification-required"));
        }
        Ok(())
    }

    /// Admits verified immutable bytes; the source path is not part of identity.
    /// # Errors
    /// All validation precedes registry mutation; replay returns the original receipt.
    pub fn admit_artifact(
        &self,
        source: &Path,
        definition: ArtifactAdmission,
        expected: RevisionRef,
    ) -> Result<(AdmittedArtifact, CommitReceipt), LocalInferenceError> {
        self.admit_artifact_controlled(
            source,
            definition,
            expected,
            &VerificationControl::new(Duration::from_secs(30)),
        )
    }

    /// Explicit cancellation/deadline variant, with the same exact receipt semantics.
    /// # Errors
    /// Cancellation, timeout or invalid input cannot create an admitted entry.
    pub fn admit_artifact_controlled(
        &self,
        source: &Path,
        definition: ArtifactAdmission,
        expected: RevisionRef,
        control: &VerificationControl,
    ) -> Result<(AdmittedArtifact, CommitReceipt), LocalInferenceError> {
        definition.validate()?;
        control.check()?;
        let _lock = self.lock(true, control)?;
        let reference = identity("model/artifact/1", &definition)?;
        let store = self.store()?;
        let (revision, mut image) = read_image(&store)?;
        if let Some(existing) = image.artifacts.iter().find(|v| v.artifact_ref == reference) {
            return Ok((existing.clone(), receipt(&store, &reference)?));
        }
        if revision != expected {
            return Err(err("admission-conflict"));
        }
        if image.artifacts.len() >= MAX_ADMITTED_ARTIFACTS {
            return Err(err("capacity-exceeded"));
        }
        let mut verified = crate::verification::verify(source, &definition, control)?;
        let value = AdmittedArtifact {
            artifact_ref: reference,
            definition,
            architecture: verified.architecture.clone(),
        };
        image.artifacts.push(value.clone());
        image
            .artifacts
            .sort_by(|a, b| a.artifact_ref.cmp(&b.artifact_ref));
        let prepared = prepare(&image, &value.artifact_ref, expected)?;
        crate::verification::place(
            &mut verified,
            &self.object_path(&value.definition.sha256),
            &value.definition,
            control,
        )?;
        let result = commit(&store, &prepared)?;
        Ok((value, result))
    }

    /// Atomically admits an exact definition. Does not start its implementation.
    /// # Errors
    /// Unknown references, unavailable bytes, incompatible target and stale revision reject.
    pub fn admit_runtime(
        &self,
        definition: RuntimeAdmission,
        expected: RevisionRef,
    ) -> Result<(AdmittedRuntime, CommitReceipt), LocalInferenceError> {
        self.admit_runtime_controlled(
            definition,
            expected,
            &VerificationControl::new(Duration::from_secs(30)),
        )
    }

    /// Controlled exact runtime definition admission, never execution.
    /// # Errors
    /// Validation, cancellation and verification failures preserve canonical state.
    pub fn admit_runtime_controlled(
        &self,
        definition: RuntimeAdmission,
        expected: RevisionRef,
        control: &VerificationControl,
    ) -> Result<(AdmittedRuntime, CommitReceipt), LocalInferenceError> {
        definition.validate()?;
        control.check()?;
        let _lock = self.lock(true, control)?;
        let store = self.store()?;
        let (revision, mut image) = read_image(&store)?;
        let value = AdmittedRuntime {
            runtime_ref: identity("runtime/definition/1", &definition)?,
            model_config_ref: definition.model_configuration.identity()?,
            runtime_config_ref: definition.runtime_configuration.identity()?,
            definition,
        };
        if let Some(existing) = image
            .runtimes
            .iter()
            .find(|v| v.runtime_ref == value.runtime_ref)
        {
            return Ok((existing.clone(), receipt(&store, &value.runtime_ref)?));
        }
        if expected != revision {
            return Err(err("admission-conflict"));
        }
        if image.runtimes.len() >= MAX_ADMITTED_RUNTIMES {
            return Err(err("capacity-exceeded"));
        }
        self.verify_runtime_definition(&image, &value.definition, control)?;
        image.runtimes.push(value.clone());
        image
            .runtimes
            .sort_by(|a, b| a.runtime_ref.cmp(&b.runtime_ref));
        let prepared = prepare(&image, &value.runtime_ref, expected)?;
        Ok((value, commit(&store, &prepared)?))
    }

    fn verify_runtime_definition(
        &self,
        image: &Image,
        definition: &RuntimeAdmission,
        control: &VerificationControl,
    ) -> Result<(), LocalInferenceError> {
        let artifact = |reference: &str| {
            image
                .artifacts
                .iter()
                .find(|v| v.artifact_ref == reference)
                .ok_or_else(|| err("artifact-not-found"))
        };
        let model = artifact(&definition.model_artifact_ref)?;
        if let Some(distribution) = image
            .distributions
            .iter()
            .find(|d| d.distribution_ref == definition.implementation_ref)
        {
            if model.definition.format != ArtifactFormat::GgufV3
                || model.architecture != definition.model_architecture
            {
                return Err(err("unsupported-runtime"));
            }
            self.verify_distribution_definition(image, &distribution.definition, control)?;
            let implementation_bytes =
                distribution
                    .definition
                    .files
                    .iter()
                    .try_fold(0u64, |sum, file| {
                        sum.checked_add(artifact(&file.artifact_ref)?.definition.bytes)
                            .ok_or_else(|| err("capacity-exceeded"))
                    })?;
            if model.definition.bytes.saturating_add(implementation_bytes)
                >= definition.runtime_configuration.memory_bytes
            {
                return Err(err("invalid-configuration"));
            }
            crate::verification::verify(
                &self.object_path(&model.definition.sha256),
                &model.definition,
                control,
            )?;
            return Ok(());
        }
        let implementation = artifact(&definition.implementation_ref)?;
        if model.definition.format != ArtifactFormat::GgufV3
            || implementation.definition.format != ArtifactFormat::StaticElf64
            || model.architecture != definition.model_architecture
            || implementation.architecture != std::env::consts::ARCH
            || std::env::consts::OS != "linux"
        {
            return Err(err("unsupported-runtime"));
        }
        if model
            .definition
            .bytes
            .saturating_add(implementation.definition.bytes)
            >= definition.runtime_configuration.memory_bytes
        {
            return Err(err("invalid-configuration"));
        }
        for item in [model, implementation] {
            crate::verification::verify(
                &self.object_path(&item.definition.sha256),
                &item.definition,
                control,
            )
            .map_err(|error| match error.code() {
                "artifact-not-found" => err("artifact-unavailable"),
                "digest-mismatch" | "unsupported-artifact" => err("artifact-corrupt"),
                _ => error,
            })?;
        }
        Ok(())
    }

    fn object_path(&self, hash: &str) -> PathBuf {
        self.root.join("objects").join(hash)
    }

    fn store(&self) -> Result<CommitStore<RedbBackend>, LocalInferenceError> {
        let path = self.root.join("admission.redb");
        if !path.try_exists().map_err(|_| err("registry-unavailable"))? {
            return Err(err("registry-missing"));
        }
        RedbBackend::open_existing(path)
            .map(CommitStore::new)
            .map_err(|error| graph_error(&error))
    }

    fn lock(
        &self,
        write: bool,
        control: &VerificationControl,
    ) -> Result<File, LocalInferenceError> {
        open_registry_directory(&self.root)?;
        let path = self.root.join("admission.lock");
        if !fs::symlink_metadata(&path)
            .map_err(|_| err("registry-missing"))?
            .file_type()
            .is_file()
        {
            return Err(err("registry-corrupt"));
        }
        let file = File::open(path).map_err(|_| err("registry-unavailable"))?;
        loop {
            control.check()?;
            let result = if write {
                file.try_lock()
            } else {
                file.try_lock_shared()
            };
            match result {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => return Err(err("registry-unavailable")),
            }
        }
    }
}

fn read_image(
    store: &CommitStore<RedbBackend>,
) -> Result<(RevisionRef, Image), LocalInferenceError> {
    let Some(current) = store.current(DOMAIN).map_err(|error| graph_error(&error))? else {
        return Ok((RevisionRef::default(), Image::default()));
    };
    if current.payload().len() > MAX_REGISTRY_PAYLOAD_BYTES {
        return Err(err("capacity-exceeded"));
    }
    let image: Image =
        serde_json::from_slice(current.payload()).map_err(|_| err("registry-corrupt"))?;
    if image.artifacts.len() > MAX_ADMITTED_ARTIFACTS
        || image.distributions.len() > 32
        || image.runtimes.len() > MAX_ADMITTED_RUNTIMES
    {
        return Err(err("capacity-exceeded"));
    }
    let receipt = receipt(store, current.operation_id())?;
    validate_image(&image)?;
    Ok((receipt.committed_revision, image))
}

fn validate_image(image: &Image) -> Result<(), LocalInferenceError> {
    let mut references = std::collections::BTreeSet::new();
    for artifact in &image.artifacts {
        artifact
            .definition
            .validate()
            .map_err(|_| err("registry-corrupt"))?;
        if artifact.artifact_ref != identity("model/artifact/1", &artifact.definition)?
            || !safe_identifier(&artifact.architecture)
            || !references.insert(&artifact.artifact_ref)
        {
            return Err(err("registry-corrupt"));
        }
    }
    let mut runtimes = std::collections::BTreeSet::new();
    let mut implementations = references.clone();
    for distribution in &image.distributions {
        distribution
            .definition
            .validate(&image.artifacts)
            .map_err(|_| err("registry-corrupt"))?;
        if distribution.distribution_ref
            != identity("runtime/distribution/1", &distribution.definition)?
            || !implementations.insert(&distribution.distribution_ref)
        {
            return Err(err("registry-corrupt"));
        }
    }
    for runtime in &image.runtimes {
        runtime
            .definition
            .validate()
            .map_err(|_| err("registry-corrupt"))?;
        if runtime.runtime_ref != identity("runtime/definition/1", &runtime.definition)?
            || runtime.model_config_ref != runtime.definition.model_configuration.identity()?
            || runtime.runtime_config_ref != runtime.definition.runtime_configuration.identity()?
            || !references.contains(&runtime.definition.model_artifact_ref)
            || !implementations.contains(&runtime.definition.implementation_ref)
            || !runtimes.insert(&runtime.runtime_ref)
        {
            return Err(err("registry-corrupt"));
        }
    }
    Ok(())
}

fn prepare(
    image: &Image,
    operation: &str,
    revision: RevisionRef,
) -> Result<zixcel_revision::PreparedCommit, LocalInferenceError> {
    let bytes = serde_json::to_vec(image).map_err(|_| err("registry-corrupt"))?;
    if bytes.len() > MAX_REGISTRY_PAYLOAD_BYTES {
        return Err(err("capacity-exceeded"));
    }
    CommitIntent {
        domain: DOMAIN.into(),
        operation_id: operation.into(),
        parents: revision.commit.iter().cloned().collect(),
        expected_revision: revision,
        payload: bytes,
    }
    .prepare()
    .map_err(|_| err("capacity-exceeded"))
}

fn commit<B: zixcel_revision::Backend>(
    store: &CommitStore<B>,
    prepared: &zixcel_revision::PreparedCommit,
) -> Result<CommitReceipt, LocalInferenceError> {
    match store.commit(prepared) {
        CommitOutcome::Committed(receipt) | CommitOutcome::NoChange(receipt) => Ok(receipt),
        CommitOutcome::Conflict(_) | CommitOutcome::Rejected(_) => Err(err("admission-conflict")),
        CommitOutcome::Failure(error) => Err(graph_error(&error)),
    }
}
fn receipt(
    store: &CommitStore<RedbBackend>,
    operation: &str,
) -> Result<CommitReceipt, LocalInferenceError> {
    store
        .receipt(DOMAIN, operation)
        .map_err(|error| graph_error(&error))?
        .ok_or_else(|| err("registry-corrupt"))
}
fn graph_error(error: &Failure) -> LocalInferenceError {
    err(match error {
        Failure::Corrupt => "registry-corrupt",
        Failure::RecoveryRequired => "registry-recovery-required",
        Failure::Capacity => "capacity-exceeded",
        Failure::DeliveryUnknown => "delivery-unknown",
        Failure::Rejected(_) => "admission-conflict",
        Failure::Storage => "registry-unavailable",
    })
}
pub(crate) fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn err(code: &'static str) -> LocalInferenceError {
    LocalInferenceError::new(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zixcel_revision::{Backend, CommitState, MemoryBackend};

    #[test]
    fn backend_recovery_requirement_is_not_logical_corruption() {
        assert_eq!(
            graph_error(&Failure::RecoveryRequired).code(),
            "registry-recovery-required"
        );
        assert_eq!(graph_error(&Failure::Corrupt).code(), "registry-corrupt");
    }

    struct InterruptedBackend(MemoryBackend);
    impl Backend for InterruptedBackend {
        fn read<T>(&self, read: impl FnOnce(&CommitState) -> T) -> Result<T, Failure> {
            self.0.read(read)
        }
        fn write<T>(
            &self,
            change: impl FnOnce(&mut CommitState) -> Result<T, Failure>,
        ) -> Result<T, Failure> {
            self.0.write(|state| {
                change(state)?;
                Err(Failure::Storage)
            })
        }
    }

    #[test]
    fn graph_transaction_interruption_never_publishes_owner_image_or_receipt() {
        let store = CommitStore::new(InterruptedBackend(MemoryBackend::default()));
        let proposal = prepare(&Image::default(), &"a".repeat(64), RevisionRef::default())
            .expect("owner preparation");
        assert_eq!(
            commit(&store, &proposal)
                .expect_err("injected storage failure")
                .code(),
            "registry-unavailable"
        );
        assert!(store.current(DOMAIN).expect("read").is_none());
        assert!(
            store
                .receipt(DOMAIN, &"a".repeat(64))
                .expect("receipt read")
                .is_none()
        );
        assert_eq!(store.stats().expect("stats").committed, 0);
    }
}
