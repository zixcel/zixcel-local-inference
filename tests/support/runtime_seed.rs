use sha2::{Digest, Sha256};
use std::fs;
use zixcel_local_inference::{
    ArtifactAdmission, ArtifactFormat, ArtifactProvenance, ModelConfiguration, RuntimeAdmission,
    RuntimeConfiguration, RuntimeRegistry,
};
pub fn descriptor(bytes: &[u8], format: ArtifactFormat) -> ArtifactAdmission {
    ArtifactAdmission {
        sha256: (Sha256::digest(bytes)).iter().fold(
            String::with_capacity(64),
            |mut encoded, byte| {
                use std::fmt::Write as _;
                write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
                encoded
            },
        ),
        bytes: bytes.len() as u64,
        format,
        provenance: ArtifactProvenance {
            publisher: "zixcel-test-author".into(),
            license: "MIT OR Apache-2.0".into(),
            source: "urn:zixcel:test:format-admission".into(),
            revision: "test-only-1".into(),
        },
    }
}

#[path = "admission_formats.rs"]
mod admission_formats;
pub use admission_formats::{gguf, static_elf};

pub fn seed_runtime(root: &std::path::Path) -> (RuntimeRegistry, RuntimeAdmission) {
    let registry =
        RuntimeRegistry::provision(&root.join("registry")).expect("fixture operation succeeds");
    let mut refs = Vec::new();
    for (name, bytes, format) in [
        ("model", gguf(1.0), ArtifactFormat::GgufV3),
        ("binary", static_elf(), ArtifactFormat::StaticElf64),
    ] {
        let source = root.join(name);
        fs::write(&source, &bytes).expect("fixture operation succeeds");
        let revision = registry
            .snapshot()
            .expect("fixture operation succeeds")
            .revision;
        refs.push(
            registry
                .admit_artifact(&source, descriptor(&bytes, format), revision)
                .expect("fixture operation succeeds")
                .0
                .artifact_ref,
        );
    }
    (
        registry,
        RuntimeAdmission {
            model_artifact_ref: refs[0].clone(),
            implementation_ref: refs[1].clone(),
            model_configuration: ModelConfiguration {
                context_tokens: 512,
                batch_tokens: 64,
            },
            runtime_configuration: RuntimeConfiguration {
                threads: 1,
                memory_bytes: 32 * 1024 * 1024,
            },
            engine: "test-format-fixture".into(),
            engine_revision: "test-only-1".into(),
            model_architecture: "test-fixture".into(),
        },
    )
}
