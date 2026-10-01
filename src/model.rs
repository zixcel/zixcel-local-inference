use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignedCatalog {
    pub schema: String,
    pub key_id: String,
    pub payload_base64: String,
    pub signature_base64: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Catalog {
    pub schema: String,
    pub catalog_id: String,
    pub sequence: u64,
    pub models: Vec<ModelDescriptor>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelDescriptor {
    pub id: String,
    pub display_name: String,
    pub publisher: String,
    pub license_spdx: String,
    pub parameter_count_millions: u32,
    pub estimated_q4_bytes: u64,
    pub languages: Vec<String>,
    pub capabilities: Vec<String>,
    pub recommendation_rank: u16,
    pub releases: Vec<ReleaseDescriptor>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseDescriptor {
    pub id: String,
    pub recommended: bool,
    pub artifacts: Vec<ArtifactDescriptor>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactDescriptor {
    pub id: String,
    pub filename: String,
    pub kind: String,
    pub format: String,
    pub quantization: Option<String>,
    pub priority: u16,
    pub runtime_engines: Vec<String>,
    pub source: ArtifactSource,
    pub integrity: ArtifactIntegrity,
    pub approval: Approval,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactSource {
    pub repository: String,
    pub revision: String,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactIntegrity {
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Approval {
    pub state: String,
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedModel {
    pub source_id: String,
    pub source_priority: u16,
    pub catalog_id: String,
    pub catalog_sequence: u64,
    pub model: ModelDescriptor,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub source_id: String,
    pub catalog_id: String,
    pub catalog_sequence: u64,
    pub id: String,
    pub display_name: String,
    pub publisher: String,
    pub license_spdx: String,
    pub parameter_count_millions: u32,
    pub estimated_q4_bytes: u64,
    pub languages: Vec<String>,
    pub capabilities: Vec<String>,
    pub recommendation_rank: u16,
    pub release_state: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Selection {
    pub format: Option<String>,
    pub runtime_engine: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionPlan {
    pub schema: String,
    pub source_id: String,
    pub catalog_id: String,
    pub catalog_sequence: u64,
    pub model_id: String,
    pub release_id: String,
    pub artifact_id: String,
    pub artifact_kind: String,
    pub format: String,
    pub quantization: Option<String>,
    pub runtime_engines: Vec<String>,
    pub source_repository: String,
    pub source_revision: String,
    pub source_url: String,
    pub expected_sha256: String,
    pub expected_bytes: u64,
    pub transport_owner: String,
    pub automatic_execution: bool,
    pub state: String,
    pub destination: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogDeliveryRequest {
    pub schema: String,
    pub source_id: String,
    pub method: String,
    pub url: String,
    pub transport_owner: String,
    pub maximum_response_bytes: u64,
    pub state: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallationStatus {
    pub schema: String,
    pub source_id: String,
    pub catalog_id: String,
    pub catalog_sequence: u64,
    pub model_id: String,
    pub display_name: String,
    pub release_id: String,
    pub artifact_id: String,
    pub artifact_kind: String,
    pub format: String,
    pub artifact_path: PathBuf,
    pub artifact_sha256: String,
    pub artifact_bytes: u64,
    pub runtime_engines: Vec<String>,
    pub state: String,
    pub inference_ready: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledModel {
    pub model_id: String,
    pub release_id: String,
    pub state: String,
    pub artifact_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeRoute {
    pub schema: String,
    pub id: String,
    pub engine: String,
    pub protocol: String,
    pub endpoint: String,
    pub capabilities: Vec<String>,
    pub enabled: bool,
}
