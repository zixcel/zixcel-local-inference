use sha2::{Digest, Sha256};
use std::{fs, time::Duration};
use zixcel_local_inference::{VerificationControl, model_bundle::VerifiedModelBundle};
fn hash(bytes: &[u8]) -> String {
    bytes_hash(&Sha256::digest(bytes))
}
fn bytes_hash(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut value = String::new();
    for byte in bytes {
        write!(value, "{byte:02x}").expect("string write");
    }
    value
}
fn manifest() -> serde_json::Value {
    serde_json::json!({"schema":"zixcel://local-inference/model-bundle/v1", "format":"openvino-ir",
        "sourceRevision":"a".repeat(40), "license":"Apache-2.0", "files":[
            {"path":"model.bin","sha256":hash(b"weights"),"bytes":7},
            {"path":"model.xml","sha256":hash(b"graph"),"bytes":5}]})
}
fn files(root: &std::path::Path) {
    fs::write(root.join("model.bin"), b"weights").expect("weights");
    fs::write(root.join("model.xml"), b"graph").expect("graph");
}
fn verify(
    root: &std::path::Path,
    value: &serde_json::Value,
) -> Result<VerifiedModelBundle, zixcel_local_inference::LocalInferenceError> {
    let document = serde_json::to_vec(value).expect("manifest");
    VerifiedModelBundle::verify(
        root,
        &document,
        &hash(&document),
        &VerificationControl::new(Duration::from_secs(2)),
    )
}
#[test]
fn observes_complete_identity_without_mutation_or_execution() {
    let directory = tempfile::tempdir().expect("bundle");
    files(directory.path());
    let bundle = verify(directory.path(), &manifest()).expect("verified bundle");
    assert_eq!(bundle.root(), directory.path());
    assert_eq!(bundle.manifest().files.len(), 2);
    bundle
        .revalidate(&VerificationControl::new(Duration::from_secs(2)))
        .expect("same bytes");
    assert_eq!(
        fs::read_dir(directory.path()).expect("inventory").count(),
        2
    );
}
#[test]
fn expected_document_hash_is_external_and_not_self_asserted() {
    let directory = tempfile::tempdir().expect("bundle");
    files(directory.path());
    let document = serde_json::to_vec(&manifest()).expect("document");
    assert!(
        VerifiedModelBundle::verify(
            directory.path(),
            &document,
            &"0".repeat(64),
            &VerificationControl::new(Duration::from_secs(2))
        )
        .is_err()
    );
}
#[test]
fn rejects_missing_extra_corrupt_and_mutated_files() {
    let directory = tempfile::tempdir().expect("bundle");
    files(directory.path());
    let bundle = verify(directory.path(), &manifest()).expect("initial");
    fs::write(directory.path().join("model.bin"), b"changed").expect("changed");
    assert!(
        bundle
            .revalidate(&VerificationControl::new(Duration::from_secs(2)))
            .is_err()
    );
    files(directory.path());
    fs::write(directory.path().join("extra.json"), b"{}").expect("extra");
    assert!(verify(directory.path(), &manifest()).is_err());
    fs::remove_file(directory.path().join("extra.json")).expect("remove extra");
    fs::remove_file(directory.path().join("model.xml")).expect("missing");
    assert!(verify(directory.path(), &manifest()).is_err());
}
#[test]
fn rejects_traversal_duplicates_unknown_fields_unbounded_sizes_and_mutable_revision() {
    let directory = tempfile::tempdir().expect("bundle");
    files(directory.path());
    for path in [
        "../model.bin",
        "/model.bin",
        "folder/model.bin",
        ".hidden",
        "model.xml",
        "MODEL.XML",
    ] {
        let mut value = manifest();
        value["files"][0]["path"] = path.into();
        assert!(verify(directory.path(), &value).is_err(), "{path}");
    }
    for revision in ["main", "v1", "z".repeat(40).as_str()] {
        let mut value = manifest();
        value["sourceRevision"] = revision.into();
        assert!(verify(directory.path(), &value).is_err());
    }
    let mut value = manifest();
    value["arbitrary"] = true.into();
    assert!(verify(directory.path(), &value).is_err());
    let mut value = manifest();
    value["files"][0]["bytes"] = u64::MAX.into();
    assert!(verify(directory.path(), &value).is_err());
}
#[test]
fn zero_length_auxiliary_weights_are_valid_only_with_matching_hash_and_nonempty_bundle() {
    let directory = tempfile::tempdir().expect("bundle");
    files(directory.path());
    fs::write(directory.path().join("model.bin"), b"").expect("empty bin");
    let mut value = manifest();
    value["files"][0]["bytes"] = 0.into();
    value["files"][0]["sha256"] = hash(b"").into();
    assert!(verify(directory.path(), &value).is_ok());
}
#[test]
fn cancelled_and_expired_observations_never_change_bundle() {
    let directory = tempfile::tempdir().expect("bundle");
    files(directory.path());
    let document = serde_json::to_vec(&manifest()).expect("manifest");
    let control = VerificationControl::new(Duration::from_secs(2));
    control.cancel();
    assert_eq!(
        VerifiedModelBundle::verify(directory.path(), &document, &hash(&document), &control)
            .expect_err("cancelled")
            .code(),
        "cancelled"
    );
    let control = VerificationControl::new(Duration::ZERO);
    assert_eq!(
        VerifiedModelBundle::verify(directory.path(), &document, &hash(&document), &control)
            .expect_err("expired")
            .code(),
        "timeout"
    );
    assert_eq!(
        fs::read(directory.path().join("model.bin")).expect("unchanged"),
        b"weights"
    );
}
#[cfg(unix)]
#[test]
fn rejects_symlink_file_and_directory() {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().expect("bundle");
    files(directory.path());
    fs::remove_file(directory.path().join("model.bin")).expect("remove");
    symlink("model.xml", directory.path().join("model.bin")).expect("symlink");
    assert!(verify(directory.path(), &manifest()).is_err());
    fs::remove_file(directory.path().join("model.bin")).expect("remove link");
    fs::create_dir(directory.path().join("model.bin")).expect("directory");
    assert!(verify(directory.path(), &manifest()).is_err());
}
