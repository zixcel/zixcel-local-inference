use sha2::{Digest, Sha256};
use std::fs;
use zixcel_local_inference::{
    ArtifactAdmission, ArtifactFormat, ArtifactProvenance, DistributionAlias, DistributionFile,
    HostCompatibility, RuntimeDistribution, RuntimeRegistry,
};

// Authored ELF metadata fixture, never executed and not a Ready/model-load proof.
fn elf(runpath: &str, needed: &str) -> Vec<u8> {
    let strings = format!("\0{needed}\0{runpath}\0");
    let mut bytes = vec![0; 1024];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    bytes[16..18].copy_from_slice(&3u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
    bytes[24..32].copy_from_slice(&256u64.to_le_bytes());
    bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
    bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
    bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&3u16.to_le_bytes());
    for (i, kind, offset, length) in [(0, 1u32, 0u64, 1024u64), (1, 2, 320, 80), (2, 3, 256, 28)] {
        let at = 64 + i * 56;
        bytes[at..at + 4].copy_from_slice(&kind.to_le_bytes());
        bytes[at + 4] = 5;
        bytes[at + 8..at + 16].copy_from_slice(&offset.to_le_bytes());
        bytes[at + 16..at + 24].copy_from_slice(&offset.to_le_bytes());
        bytes[at + 32..at + 40].copy_from_slice(&length.to_le_bytes());
        bytes[at + 40..at + 48].copy_from_slice(&length.to_le_bytes());
    }
    bytes[256..284].copy_from_slice(b"/lib64/ld-linux-x86-64.so.2\0");
    for (i, (tag, value)) in [
        (5u64, 512u64),
        (10, strings.len() as u64),
        (1, 1),
        (29, needed.len() as u64 + 2),
        (0, 0),
    ]
    .iter()
    .enumerate()
    {
        let at = 320 + i * 16;
        bytes[at..at + 8].copy_from_slice(&tag.to_le_bytes());
        bytes[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
    }
    bytes[512..512 + strings.len()].copy_from_slice(strings.as_bytes());
    bytes
}

#[test]
fn exact_distribution_replay_read_purity_alias_safety_and_dependency_rejection() {
    let temp = tempfile::tempdir().expect("isolated");
    let registry = RuntimeRegistry::provision(&temp.path().join("registry")).expect("provision");
    let admit = |bytes: Vec<u8>| {
        let source = temp.path().join("candidate");
        fs::write(&source, &bytes).expect("fixture");
        registry.admit_artifact(
            &source,
            ArtifactAdmission {
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                bytes: bytes.len() as u64,
                format: ArtifactFormat::DynamicElf64,
                provenance: ArtifactProvenance {
                    publisher: "fixture".into(),
                    license: "MIT".into(),
                    source: "urn:test:distribution".into(),
                    revision: "test".into(),
                },
            },
            registry.snapshot().expect("snapshot").revision,
        )
    };
    for unsafe_path in ["/tmp", "$ORIGIN/..", "", "$ORIGIN:/usr/lib", "."] {
        assert!(admit(elf(unsafe_path, "libc.so.6")).is_err());
        assert!(registry.snapshot().expect("pure").artifacts.is_empty());
    }
    let (artifact, _) = admit(elf("$ORIGIN", "libc.so.6")).expect("dynamic artifact");
    let definition = RuntimeDistribution {
        executable: "engine".into(),
        files: vec![DistributionFile {
            path: "engine".into(),
            artifact_ref: artifact.artifact_ref,
        }],
        aliases: vec![],
        modules: vec![],
        host: HostCompatibility {
            os: "linux".into(),
            architecture: "x86_64".into(),
            interpreter: "/lib64/ld-linux-x86-64.so.2".into(),
            libraries: vec!["libc.so.6".into()],
            symbol_versions: std::collections::BTreeMap::default(),
        },
    };
    let before = registry.snapshot().expect("before").revision;
    let (distribution, receipt) = registry
        .admit_distribution(definition.clone(), before.clone())
        .expect("closed");
    let (_, replay) = registry
        .admit_distribution(definition.clone(), before)
        .expect("original receipt");
    assert_eq!(receipt, replay);
    let db = temp.path().join("registry/admission.redb");
    let original = fs::read(&db).expect("before read");
    registry
        .verify_distribution(&distribution.distribution_ref)
        .expect("read exact bytes");
    assert_eq!(original, fs::read(&db).expect("after read"));
    for aliases in [
        vec![DistributionAlias {
            path: "outside".into(),
            target: "../engine".into(),
        }],
        vec![DistributionAlias {
            path: "cycle".into(),
            target: "cycle".into(),
        }],
    ] {
        let mut bad = definition.clone();
        bad.aliases = aliases;
        assert!(
            registry
                .admit_distribution(bad, registry.snapshot().expect("revision").revision)
                .is_err()
        );
    }
    let mut bad = definition;
    bad.host.libraries = vec!["libm.so.6".into()];
    assert_eq!(
        registry
            .admit_distribution(bad, registry.snapshot().expect("revision").revision)
            .expect_err("unresolved")
            .code(),
        "distribution-dependency-unresolved"
    );
    assert_eq!(original, fs::read(&db).expect("negative read purity"));
}
