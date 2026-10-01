use std::fs;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use zixcel_local_inference::{
    Approval, ArtifactDescriptor, ArtifactIntegrity, ArtifactSource, CATALOG_SCHEMA, Catalog,
    ModelDescriptor, ReleaseDescriptor, SIGNED_CATALOG_SCHEMA, SignedCatalog,
};

pub const MODEL_BYTES: &[u8] = b"generic-model-fixture\n";

pub fn catalog(sequence: u64, models: Vec<ModelDescriptor>) -> Catalog {
    Catalog {
        schema: CATALOG_SCHEMA.to_owned(),
        catalog_id: "test-catalog".to_owned(),
        sequence,
        models,
    }
}

pub fn model(id: &str, approval: &str) -> ModelDescriptor {
    ModelDescriptor {
        id: id.to_owned(),
        display_name: format!("Display {id}"),
        publisher: "test-publisher".to_owned(),
        license_spdx: "Apache-2.0".to_owned(),
        parameter_count_millions: 100,
        estimated_q4_bytes: 75_000_000,
        languages: vec!["en".to_owned()],
        capabilities: vec!["classification".to_owned()],
        recommendation_rank: 10,
        releases: vec![ReleaseDescriptor {
            id: "release-1".to_owned(),
            recommended: true,
            artifacts: vec![ArtifactDescriptor {
                id: "artifact-1".to_owned(),
                filename: "model.gguf".to_owned(),
                kind: "model-weights".to_owned(),
                format: "gguf".to_owned(),
                quantization: Some("q4_k_m".to_owned()),
                priority: 1,
                runtime_engines: vec!["llama.cpp".to_owned()],
                source: ArtifactSource {
                    repository: "publisher/model".to_owned(),
                    revision: "revision-1".to_owned(),
                    url: "https://models.example.test/model.gguf".to_owned(),
                },
                integrity: ArtifactIntegrity {
                    sha256: format!("{:x}", Sha256::digest(MODEL_BYTES)),
                    bytes: u64::try_from(MODEL_BYTES.len()).expect("fixture length"),
                },
                approval: Approval {
                    state: approval.to_owned(),
                    evidence: vec!["test-review".to_owned()],
                },
            }],
        }],
    }
}

pub fn write_signed(path: &Path, catalog: &Catalog, seed: u8) -> (String, String) {
    let signing_key = SigningKey::from_bytes(&[seed; 32]);
    let payload = serde_json::to_vec_pretty(catalog).expect("catalog payload");
    let signature = signing_key.sign(&payload);
    let key_id = format!("test-key-{seed}");
    let envelope = SignedCatalog {
        schema: SIGNED_CATALOG_SCHEMA.to_owned(),
        key_id: key_id.clone(),
        payload_base64: STANDARD.encode(&payload),
        signature_base64: STANDARD.encode(signature.to_bytes()),
    };
    fs::write(
        path,
        serde_json::to_vec_pretty(&envelope).expect("signed catalog"),
    )
    .expect("write catalog");
    (
        key_id,
        STANDARD.encode(signing_key.verifying_key().to_bytes()),
    )
}
