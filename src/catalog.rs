use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, VerifyingKey};
use http::Uri;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fsutil::{
    MAX_DOCUMENT_BYTES, open_registry_directory, prepare_directory, read_regular_file,
    safe_identifier, validate_absolute, write_new,
};
use crate::{
    ACQUISITION_SCHEMA, CATALOG_SCHEMA, Candidate, Catalog, CatalogDeliveryRequest,
    DELIVERY_SCHEMA, LocalInferenceError, ModelDescriptor, ResolvedModel, SIGNED_CATALOG_SCHEMA,
    SOURCE_SCHEMA, Selection, SignedCatalog,
};

const MAX_CATALOG_MODELS: usize = 10_000;
const MAX_RELEASES_PER_MODEL: usize = 256;
const MAX_ARTIFACTS_PER_RELEASE: usize = 256;
const MAX_VALUES_PER_FIELD: usize = 256;
const MAX_TEXT_BYTES: usize = 4_096;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceRecord {
    pub schema: String,
    pub id: String,
    pub priority: u16,
    pub kind: String,
    pub location: String,
    pub key_id: String,
    pub public_key_base64: String,
}

#[derive(Clone, Debug)]
pub struct CatalogStore {
    state_root: PathBuf,
}

impl CatalogStore {
    /// Opens an existing catalog. Reading never provisions missing storage.
    ///
    /// # Errors
    /// Returns typed absence or invalid storage without attempting repair.
    pub fn open_existing(state_root: &Path) -> Result<Self, LocalInferenceError> {
        let state_root = open_registry_directory(state_root)?;
        open_registry_directory(&state_root.join("sources"))?;
        open_registry_directory(&state_root.join("catalogs"))?;
        Ok(Self { state_root })
    }

    /// Explicitly provisions catalog storage; does not select or download a model.
    /// # Errors
    /// Unsafe paths and storage failures reject provisioning.
    pub fn provision(state_root: &Path) -> Result<Self, LocalInferenceError> {
        let state_root = prepare_directory(state_root)?;
        prepare_directory(&state_root.join("sources"))?;
        prepare_directory(&state_root.join("catalogs"))?;
        Ok(Self { state_root })
    }

    /// Registers one immutable, local, signed-catalog source.
    ///
    /// # Errors
    /// Returns an error for invalid identifiers, paths, keys, or duplicate sources.
    pub fn add_file_source(
        &self,
        id: &str,
        priority: u16,
        catalog_path: &Path,
        key_id: &str,
        public_key_base64: &str,
    ) -> Result<SourceRecord, LocalInferenceError> {
        validate_absolute(catalog_path)?;
        self.add_source(SourceRecord {
            schema: SOURCE_SCHEMA.to_owned(),
            id: id.to_owned(),
            priority,
            kind: "file".to_owned(),
            location: catalog_path.to_string_lossy().into_owned(),
            key_id: key_id.to_owned(),
            public_key_base64: public_key_base64.trim().to_owned(),
        })
    }

    /// Registers one immutable remote source whose bytes must be delivered by Crowsi.
    ///
    /// # Errors
    /// Returns an error for invalid identifiers, endpoints, keys, or duplicate sources.
    pub fn add_remote_source(
        &self,
        id: &str,
        priority: u16,
        endpoint: &str,
        key_id: &str,
        public_key_base64: &str,
    ) -> Result<SourceRecord, LocalInferenceError> {
        validate_network_url(endpoint)?;
        self.add_source(SourceRecord {
            schema: SOURCE_SCHEMA.to_owned(),
            id: id.to_owned(),
            priority,
            kind: "remote".to_owned(),
            location: endpoint.to_owned(),
            key_id: key_id.to_owned(),
            public_key_base64: public_key_base64.trim().to_owned(),
        })
    }

    /// Lists validated catalog sources in deterministic priority order.
    ///
    /// # Errors
    /// Returns an error when stored source records cannot be read or validated.
    pub fn sources(&self) -> Result<Vec<SourceRecord>, LocalInferenceError> {
        let directory = self.state_root.join("sources");
        let mut records = Vec::new();
        for entry in
            fs::read_dir(directory).map_err(|_| LocalInferenceError::new("state-io-failed"))?
        {
            let entry = entry.map_err(|_| LocalInferenceError::new("state-io-failed"))?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let bytes = read_regular_file(&path, MAX_DOCUMENT_BYTES)?;
            let source: SourceRecord = serde_json::from_slice(&bytes)
                .map_err(|_| LocalInferenceError::new("source-invalid"))?;
            validate_source(&source)?;
            records.push(source);
        }
        records.sort_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(records)
    }

    /// Builds a non-executing Crowsi delivery request for a remote source.
    ///
    /// # Errors
    /// Returns an error when the source is unknown or is not remote.
    pub fn delivery_request(
        &self,
        source_id: &str,
    ) -> Result<CatalogDeliveryRequest, LocalInferenceError> {
        let source = self.source(source_id)?;
        if source.kind != "remote" {
            return Err(LocalInferenceError::new("source-delivery-not-required"));
        }
        Ok(CatalogDeliveryRequest {
            schema: DELIVERY_SCHEMA.to_owned(),
            source_id: source.id,
            method: "GET".to_owned(),
            url: source.location,
            transport_owner: "crowsi".to_owned(),
            maximum_response_bytes: MAX_DOCUMENT_BYTES,
            state: "awaiting-crowsi-delivery".to_owned(),
        })
    }

    /// Verifies and stores the next signed generation for a source.
    ///
    /// # Errors
    /// Returns an error for missing delivery, signature failure, rollback, equivocation, or I/O.
    pub fn refresh(
        &self,
        source_id: &str,
        delivered_file: Option<&Path>,
    ) -> Result<Catalog, LocalInferenceError> {
        let source = self.source(source_id)?;
        let path = match (source.kind.as_str(), delivered_file) {
            ("file", None) => PathBuf::from(&source.location),
            ("file", Some(_)) => {
                return Err(LocalInferenceError::new("source-delivery-unexpected"));
            }
            ("remote", Some(path)) => path.to_path_buf(),
            ("remote", None) => return Err(LocalInferenceError::new("catalog-delivery-required")),
            _ => return Err(LocalInferenceError::new("source-invalid")),
        };
        let envelope_bytes = read_regular_file(&path, MAX_DOCUMENT_BYTES)?;
        let catalog = verify_catalog(&source, &envelope_bytes)?;
        self.persist_catalog(&source, &catalog, &envelope_bytes)?;
        Ok(catalog)
    }

    /// Resolves all current catalogs by priority without silently resolving equal-priority conflicts.
    ///
    /// # Errors
    /// Returns an error when a source is unrefreshed, invalid, or conflicts at equal priority.
    pub fn models(&self) -> Result<Vec<ResolvedModel>, LocalInferenceError> {
        let mut selected: BTreeMap<String, ResolvedModel> = BTreeMap::new();
        let sources = self.sources()?;
        for source in sources {
            let catalog = self.latest_catalog(&source)?;
            for model in catalog.models {
                let resolved = ResolvedModel {
                    source_id: source.id.clone(),
                    source_priority: source.priority,
                    catalog_id: catalog.catalog_id.clone(),
                    catalog_sequence: catalog.sequence,
                    model,
                };
                if let Some(existing) = selected.get(&resolved.model.id) {
                    if existing.source_priority == resolved.source_priority
                        && existing.model != resolved.model
                    {
                        return Err(LocalInferenceError::new("catalog-model-conflict"));
                    }
                    continue;
                }
                selected.insert(resolved.model.id.clone(), resolved);
            }
        }
        let mut models: Vec<_> = selected.into_values().collect();
        models.sort_by(|left, right| {
            left.model
                .recommendation_rank
                .cmp(&right.model.recommendation_rank)
                .then_with(|| left.model.id.cmp(&right.model.id))
        });
        Ok(models)
    }

    /// Returns the model-independent candidate projection for the current catalogs.
    ///
    /// # Errors
    /// Returns an error when current catalogs cannot be resolved.
    pub fn candidates(&self) -> Result<Vec<Candidate>, LocalInferenceError> {
        self.models()?
            .into_iter()
            .map(|resolved| {
                let approved = resolved
                    .model
                    .releases
                    .iter()
                    .flat_map(|release| &release.artifacts)
                    .any(|artifact| artifact.approval.state == "approved");
                Ok(Candidate {
                    source_id: resolved.source_id,
                    catalog_id: resolved.catalog_id,
                    catalog_sequence: resolved.catalog_sequence,
                    id: resolved.model.id,
                    display_name: resolved.model.display_name,
                    publisher: resolved.model.publisher,
                    license_spdx: resolved.model.license_spdx,
                    parameter_count_millions: resolved.model.parameter_count_millions,
                    estimated_q4_bytes: resolved.model.estimated_q4_bytes,
                    languages: resolved.model.languages,
                    capabilities: resolved.model.capabilities,
                    recommendation_rank: resolved.model.recommendation_rank,
                    release_state: if approved {
                        "installable".to_owned()
                    } else {
                        "review-required".to_owned()
                    },
                })
            })
            .collect()
    }

    /// Resolves one model from the current catalog generations.
    ///
    /// # Errors
    /// Returns an error when catalogs are invalid or the model is unknown.
    pub fn model(&self, model_id: &str) -> Result<ResolvedModel, LocalInferenceError> {
        self.models()?
            .into_iter()
            .find(|resolved| resolved.model.id == model_id)
            .ok_or_else(|| LocalInferenceError::new("model-unknown"))
    }

    /// Selects one approved artifact and creates a non-executing acquisition plan.
    ///
    /// # Errors
    /// Returns an error for unsafe paths, unknown models, or no approved compatible artifact.
    pub fn acquisition_plan(
        &self,
        model_id: &str,
        model_root: &Path,
        selection: &Selection,
    ) -> Result<crate::AcquisitionPlan, LocalInferenceError> {
        validate_absolute(model_root)?;
        let resolved = self.model(model_id)?;
        let (release, artifact) = select_artifact(&resolved.model, selection)?;
        Ok(crate::AcquisitionPlan {
            schema: ACQUISITION_SCHEMA.to_owned(),
            source_id: resolved.source_id,
            catalog_id: resolved.catalog_id,
            catalog_sequence: resolved.catalog_sequence,
            model_id: resolved.model.id.clone(),
            release_id: release.id.clone(),
            artifact_id: artifact.id.clone(),
            artifact_kind: artifact.kind.clone(),
            format: artifact.format.clone(),
            quantization: artifact.quantization.clone(),
            runtime_engines: artifact.runtime_engines.clone(),
            source_repository: artifact.source.repository.clone(),
            source_revision: artifact.source.revision.clone(),
            source_url: artifact.source.url.clone(),
            expected_sha256: artifact.integrity.sha256.clone(),
            expected_bytes: artifact.integrity.bytes,
            transport_owner: "crowsi".to_owned(),
            automatic_execution: false,
            state: "awaiting-crowsi-delivery".to_owned(),
            destination: model_root
                .join(&resolved.model.id)
                .join(&release.id)
                .join(&artifact.filename),
        })
    }

    pub(crate) fn installation_evidence(
        &self,
        source_id: &str,
        catalog_id: &str,
        catalog_sequence: u64,
        model_id: &str,
        release_id: &str,
        artifact_id: &str,
    ) -> Result<(ModelDescriptor, crate::ArtifactDescriptor), LocalInferenceError> {
        let source = self.source(source_id)?;
        let catalog = self.catalog_at(&source, catalog_sequence)?;
        if catalog.catalog_id != catalog_id {
            return Err(LocalInferenceError::new("installation-evidence-mismatch"));
        }
        let model = catalog
            .models
            .into_iter()
            .find(|model| model.id == model_id)
            .ok_or_else(|| LocalInferenceError::new("installation-evidence-mismatch"))?;
        let artifact = model
            .releases
            .iter()
            .find(|release| release.id == release_id)
            .and_then(|release| {
                release
                    .artifacts
                    .iter()
                    .find(|artifact| artifact.id == artifact_id)
            })
            .cloned()
            .ok_or_else(|| LocalInferenceError::new("installation-evidence-mismatch"))?;
        if artifact.approval.state != "approved" {
            return Err(LocalInferenceError::new("installation-evidence-mismatch"));
        }
        Ok((model, artifact))
    }

    fn add_source(&self, source: SourceRecord) -> Result<SourceRecord, LocalInferenceError> {
        validate_source(&source)?;
        let bytes = serde_json::to_vec_pretty(&source)
            .map_err(|_| LocalInferenceError::new("source-invalid"))?;
        write_new(
            &self
                .state_root
                .join("sources")
                .join(format!("{}.json", source.id)),
            &bytes,
        )?;
        Ok(source)
    }

    fn source(&self, source_id: &str) -> Result<SourceRecord, LocalInferenceError> {
        if !safe_identifier(source_id) {
            return Err(LocalInferenceError::new("source-invalid"));
        }
        self.sources()?
            .into_iter()
            .find(|source| source.id == source_id)
            .ok_or_else(|| LocalInferenceError::new("source-unknown"))
    }

    fn persist_catalog(
        &self,
        source: &SourceRecord,
        catalog: &Catalog,
        envelope: &[u8],
    ) -> Result<(), LocalInferenceError> {
        let source_directory =
            prepare_directory(&self.state_root.join("catalogs").join(&source.id))?;
        let digest = format!("{:x}", Sha256::digest(envelope));
        let existing = cached_catalog_files(&source_directory)?;
        if let Some((highest, highest_digest, highest_path)) = existing.last() {
            let previous = verify_catalog(
                source,
                &read_regular_file(highest_path, MAX_DOCUMENT_BYTES)?,
            )?;
            if previous.catalog_id != catalog.catalog_id {
                return Err(LocalInferenceError::new("catalog-identity-changed"));
            }
            if catalog.sequence < *highest {
                return Err(LocalInferenceError::new("catalog-rollback-rejected"));
            }
            if catalog.sequence == *highest {
                if digest == *highest_digest {
                    return Ok(());
                }
                return Err(LocalInferenceError::new("catalog-sequence-equivocation"));
            }
        }
        let path = source_directory.join(format!("{:020}-{digest}.json", catalog.sequence));
        write_new(&path, envelope)
    }

    fn latest_catalog(&self, source: &SourceRecord) -> Result<Catalog, LocalInferenceError> {
        let directory = self.state_root.join("catalogs").join(&source.id);
        let files = cached_catalog_files(&directory)?;
        let (_, _, path) = files
            .last()
            .ok_or_else(|| LocalInferenceError::new("catalog-not-refreshed"))?;
        let bytes = read_regular_file(path, MAX_DOCUMENT_BYTES)?;
        verify_catalog(source, &bytes)
    }

    fn catalog_at(
        &self,
        source: &SourceRecord,
        sequence: u64,
    ) -> Result<Catalog, LocalInferenceError> {
        let directory = self.state_root.join("catalogs").join(&source.id);
        let (_, _, path) = cached_catalog_files(&directory)?
            .into_iter()
            .find(|(value, _, _)| *value == sequence)
            .ok_or_else(|| LocalInferenceError::new("catalog-generation-not-found"))?;
        let bytes = read_regular_file(&path, MAX_DOCUMENT_BYTES)?;
        verify_catalog(source, &bytes)
    }
}

fn validate_source(source: &SourceRecord) -> Result<(), LocalInferenceError> {
    if source.schema != SOURCE_SCHEMA
        || !safe_identifier(&source.id)
        || !safe_identifier(&source.key_id)
        || decode_public_key(&source.public_key_base64).is_err()
    {
        return Err(LocalInferenceError::new("source-invalid"));
    }
    match source.kind.as_str() {
        "file" => validate_absolute(Path::new(&source.location)),
        "remote" => validate_network_url(&source.location),
        _ => Err(LocalInferenceError::new("source-invalid")),
    }
}

fn verify_catalog(
    source: &SourceRecord,
    envelope_bytes: &[u8],
) -> Result<Catalog, LocalInferenceError> {
    let envelope: SignedCatalog = serde_json::from_slice(envelope_bytes)
        .map_err(|_| LocalInferenceError::new("catalog-envelope-invalid"))?;
    if envelope.schema != SIGNED_CATALOG_SCHEMA || envelope.key_id != source.key_id {
        return Err(LocalInferenceError::new("catalog-envelope-invalid"));
    }
    let payload = STANDARD
        .decode(&envelope.payload_base64)
        .map_err(|_| LocalInferenceError::new("catalog-envelope-invalid"))?;
    if payload.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(LocalInferenceError::new("catalog-envelope-invalid"));
    }
    let signature_bytes = STANDARD
        .decode(&envelope.signature_base64)
        .map_err(|_| LocalInferenceError::new("catalog-signature-invalid"))?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| LocalInferenceError::new("catalog-signature-invalid"))?;
    decode_public_key(&source.public_key_base64)?
        .verify_strict(&payload, &signature)
        .map_err(|_| LocalInferenceError::new("catalog-signature-invalid"))?;
    let catalog: Catalog = serde_json::from_slice(&payload)
        .map_err(|_| LocalInferenceError::new("catalog-payload-invalid"))?;
    validate_catalog(&catalog)?;
    Ok(catalog)
}

fn decode_public_key(encoded: &str) -> Result<VerifyingKey, LocalInferenceError> {
    let bytes = STANDARD
        .decode(encoded.trim())
        .map_err(|_| LocalInferenceError::new("public-key-invalid"))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| LocalInferenceError::new("public-key-invalid"))?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| LocalInferenceError::new("public-key-invalid"))
}

fn validate_catalog(catalog: &Catalog) -> Result<(), LocalInferenceError> {
    if catalog.schema != CATALOG_SCHEMA
        || !safe_identifier(&catalog.catalog_id)
        || catalog.sequence == 0
        || catalog.models.len() > MAX_CATALOG_MODELS
    {
        return Err(LocalInferenceError::new("catalog-payload-invalid"));
    }
    let mut model_ids = BTreeSet::new();
    for model in &catalog.models {
        if !model_ids.insert(&model.id) {
            return Err(LocalInferenceError::new("catalog-model-duplicate"));
        }
        validate_model(model)?;
    }
    Ok(())
}

fn validate_model(model: &ModelDescriptor) -> Result<(), LocalInferenceError> {
    if !safe_identifier(&model.id)
        || model.display_name.trim().is_empty()
        || model.publisher.trim().is_empty()
        || model.license_spdx.trim().is_empty()
        || model.languages.is_empty()
        || model.capabilities.is_empty()
        || model.releases.len() > MAX_RELEASES_PER_MODEL
        || !valid_text(&model.display_name)
        || !valid_text(&model.publisher)
        || !valid_text(&model.license_spdx)
        || !valid_values(&model.languages)
        || !valid_values(&model.capabilities)
    {
        return Err(LocalInferenceError::new("catalog-model-invalid"));
    }
    let mut release_ids = BTreeSet::new();
    for release in &model.releases {
        if !safe_identifier(&release.id)
            || !release_ids.insert(&release.id)
            || release.artifacts.len() > MAX_ARTIFACTS_PER_RELEASE
        {
            return Err(LocalInferenceError::new("catalog-release-invalid"));
        }
        let mut artifact_ids = BTreeSet::new();
        for artifact in &release.artifacts {
            let valid_hash = artifact.integrity.sha256.len() == 64
                && artifact
                    .integrity
                    .sha256
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
            if !safe_identifier(&artifact.id)
                || !safe_identifier(&artifact.filename)
                || !artifact_ids.insert(&artifact.id)
                || artifact.integrity.bytes == 0
                || !valid_hash
                || !safe_identifier(&artifact.kind)
                || !safe_identifier(&artifact.format)
                || artifact
                    .quantization
                    .as_ref()
                    .is_some_and(|value| !safe_identifier(value))
                || artifact.runtime_engines.is_empty()
                || !valid_values(&artifact.runtime_engines)
                || !valid_text(&artifact.source.repository)
                || !valid_text(&artifact.source.revision)
                || (artifact.approval.state == "approved" && artifact.approval.evidence.is_empty())
                || !valid_values(&artifact.approval.evidence)
                || !matches!(
                    artifact.approval.state.as_str(),
                    "approved" | "review-required" | "rejected"
                )
                || validate_network_url(&artifact.source.url).is_err()
            {
                return Err(LocalInferenceError::new("catalog-artifact-invalid"));
            }
        }
    }
    Ok(())
}

fn validate_network_url(value: &str) -> Result<(), LocalInferenceError> {
    if !valid_text(value) {
        return Err(LocalInferenceError::new("url-invalid"));
    }
    let parsed: Uri = value
        .parse()
        .map_err(|_| LocalInferenceError::new("url-invalid"))?;
    let scheme = parsed
        .scheme_str()
        .ok_or_else(|| LocalInferenceError::new("url-invalid"))?;
    let authority = parsed
        .authority()
        .ok_or_else(|| LocalInferenceError::new("url-invalid"))?;
    if !matches!(scheme, "http" | "https") || authority.as_str().contains('@') {
        return Err(LocalInferenceError::new("url-invalid"));
    }
    Ok(())
}

fn valid_text(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_TEXT_BYTES
        && !value.chars().any(char::is_control)
}

fn valid_values(values: &[String]) -> bool {
    values.len() <= MAX_VALUES_PER_FIELD && values.iter().all(|value| valid_text(value))
}

fn select_artifact<'a>(
    model: &'a ModelDescriptor,
    selection: &Selection,
) -> Result<(&'a crate::ReleaseDescriptor, &'a crate::ArtifactDescriptor), LocalInferenceError> {
    let mut releases: Vec<_> = model.releases.iter().collect();
    releases.sort_by(|left, right| {
        right
            .recommended
            .cmp(&left.recommended)
            .then_with(|| right.id.cmp(&left.id))
    });
    for release in releases {
        let mut artifacts: Vec<_> = release
            .artifacts
            .iter()
            .filter(|artifact| artifact.approval.state == "approved")
            .filter(|artifact| {
                selection
                    .format
                    .as_ref()
                    .is_none_or(|format| artifact.format.eq_ignore_ascii_case(format))
            })
            .filter(|artifact| {
                selection.runtime_engine.as_ref().is_none_or(|runtime| {
                    artifact
                        .runtime_engines
                        .iter()
                        .any(|engine| engine == runtime)
                })
            })
            .collect();
        artifacts.sort_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| left.id.cmp(&right.id))
        });
        if let Some(artifact) = artifacts.first() {
            return Ok((release, artifact));
        }
    }
    Err(LocalInferenceError::new("artifact-not-approved"))
}

fn cached_catalog_files(
    directory: &Path,
) -> Result<Vec<(u64, String, PathBuf)>, LocalInferenceError> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).map_err(|_| LocalInferenceError::new("state-io-failed"))? {
        let entry = entry.map_err(|_| LocalInferenceError::new("state-io-failed"))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| LocalInferenceError::new("state-io-failed"))?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(LocalInferenceError::new("catalog-cache-invalid"));
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some((sequence, digest_with_extension)) = name.split_once('-') else {
            continue;
        };
        let Some(digest) = digest_with_extension.strip_suffix(".json") else {
            continue;
        };
        let Ok(sequence) = sequence.parse::<u64>() else {
            continue;
        };
        files.push((sequence, digest.to_owned(), entry.path()));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    Ok(files)
}
