mod support;

use std::fs;

use tempfile::tempdir;
use zixcel_local_inference::{
    CatalogStore, RuntimeRouteRegistry, Selection, install_from_file, installation_statuses,
    installed_models, remove_installation,
};

use support::{MODEL_BYTES, catalog, model, write_signed};

fn prepared_store(
    temporary: &tempfile::TempDir,
) -> (CatalogStore, std::path::PathBuf, std::path::PathBuf) {
    let state = temporary.path().join("state");
    let root = temporary.path().join("models");
    let envelope = temporary.path().join("catalog.json");
    let artifact = temporary.path().join("delivered.gguf");
    fs::write(&artifact, MODEL_BYTES).expect("artifact fixture");
    let (key_id, public_key) = write_signed(
        &envelope,
        &catalog(1, vec![model("dynamic", "approved")]),
        6,
    );
    let store = CatalogStore::provision(&state).expect("catalog store");
    store
        .add_file_source("primary", 100, &envelope, &key_id, &public_key)
        .expect("source");
    store.refresh("primary", None).expect("refresh");
    (store, root, artifact)
}

#[test]
fn resolver_install_status_and_removal_form_one_verified_lifecycle() {
    let temporary = tempdir().expect("temporary directory");
    let (store, root, artifact) = prepared_store(&temporary);
    let selection = Selection {
        format: Some("gguf".to_owned()),
        runtime_engine: Some("llama.cpp".to_owned()),
    };
    let plan = store
        .acquisition_plan("dynamic", &root, &selection)
        .expect("plan");
    assert_eq!(plan.transport_owner, "crowsi");
    assert!(!plan.automatic_execution);
    assert_eq!(plan.expected_bytes, MODEL_BYTES.len() as u64);

    let installed =
        install_from_file(&store, "dynamic", &root, &selection, &artifact).expect("install");
    assert_eq!(installed.state, "artifact-ready");
    assert!(!installed.inference_ready);
    assert_eq!(
        install_from_file(&store, "dynamic", &root, &selection, &artifact).expect("idempotent"),
        installed
    );
    assert_eq!(
        installation_statuses(&store, &root, "dynamic").expect("status"),
        vec![installed.clone()]
    );
    assert_eq!(installed_models(&store, &root).expect("installed").len(), 1);

    assert_eq!(
        remove_installation(&root, "dynamic", "release-1", "wrong")
            .expect_err("confirmation")
            .code(),
        "removal-confirmation-invalid"
    );
    remove_installation(&root, "dynamic", "release-1", "dynamic@release-1").expect("remove");
    assert!(!root.join("dynamic").exists());
}

#[test]
fn artifact_and_manifest_tampering_are_detected_against_signed_evidence() {
    let temporary = tempdir().expect("temporary directory");
    let (store, root, artifact) = prepared_store(&temporary);
    let installed = install_from_file(&store, "dynamic", &root, &Selection::default(), &artifact)
        .expect("install");
    fs::write(&installed.artifact_path, b"mutated-model-content").expect("mutate artifact");
    let manifest_path = installed
        .artifact_path
        .parent()
        .expect("parent")
        .join("installation.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest"))
            .expect("manifest json");
    manifest["artifactBytes"] = 21_u64.into();
    manifest["artifactSha256"] =
        "64c85991f4c6f70f2613ec78785d4cde2a65bb202b129dd16fc55f6cf57cf4d1".into();
    fs::write(
        manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest bytes"),
    )
    .expect("mutate manifest");
    assert_eq!(
        installation_statuses(&store, &root, "dynamic")
            .expect_err("signed evidence")
            .code(),
        "installation-evidence-mismatch"
    );
}

#[test]
fn reviewed_artifact_and_runtime_compatibility_are_mandatory() {
    let temporary = tempdir().expect("temporary directory");
    let state = temporary.path().join("state");
    let envelope = temporary.path().join("catalog.json");
    let (key_id, public_key) = write_signed(
        &envelope,
        &catalog(1, vec![model("blocked", "review-required")]),
        7,
    );
    let store = CatalogStore::provision(&state).expect("store");
    store
        .add_file_source("primary", 100, &envelope, &key_id, &public_key)
        .expect("source");
    store.refresh("primary", None).expect("refresh");
    assert_eq!(
        store
            .acquisition_plan(
                "blocked",
                &temporary.path().join("models"),
                &Selection::default()
            )
            .expect_err("review")
            .code(),
        "artifact-not-approved"
    );

    let runtime = RuntimeRouteRegistry::provision(&state).expect("runtime registry");
    runtime
        .add(
            "local-cpu",
            "llama.cpp",
            "openai-compatible",
            "http://127.0.0.1:8080/v1",
            vec!["classification".to_owned()],
        )
        .expect("runtime route");
    assert_eq!(
        runtime.route("local-cpu").expect("route").engine,
        "llama.cpp"
    );
}

#[test]
fn interrupted_copy_is_recovered_only_when_bytes_match_signed_evidence() {
    let temporary = tempdir().expect("temporary directory");
    let (store, root, artifact) = prepared_store(&temporary);
    let plan = store
        .acquisition_plan("dynamic", &root, &Selection::default())
        .expect("plan");
    fs::create_dir_all(plan.destination.parent().expect("destination parent"))
        .expect("release directory");
    fs::copy(&artifact, &plan.destination).expect("interrupted copy result");
    let recovered = install_from_file(&store, "dynamic", &root, &Selection::default(), &artifact)
        .expect("recover manifest");
    assert_eq!(recovered.artifact_path, plan.destination);
    assert_eq!(
        installation_statuses(&store, &root, "dynamic")
            .expect("status")
            .len(),
        1
    );

    let second = tempdir().expect("second temporary directory");
    let (store, root, artifact) = prepared_store(&second);
    let plan = store
        .acquisition_plan("dynamic", &root, &Selection::default())
        .expect("plan");
    fs::create_dir_all(plan.destination.parent().expect("destination parent"))
        .expect("release directory");
    fs::write(&plan.destination, b"mutated-model-fixture\n")
        .expect("wrong interrupted copy result");
    assert_eq!(
        install_from_file(&store, "dynamic", &root, &Selection::default(), &artifact,)
            .expect_err("wrong recovery")
            .code(),
        "installation-state-conflict"
    );
}

#[cfg(unix)]
#[test]
fn removal_rejects_a_release_directory_replaced_by_a_symlink() {
    use std::os::unix::fs::symlink;

    let temporary = tempdir().expect("temporary directory");
    let (store, root, artifact) = prepared_store(&temporary);
    install_from_file(&store, "dynamic", &root, &Selection::default(), &artifact).expect("install");
    let release = root.join("dynamic").join("release-1");
    let moved = root.join("dynamic").join("moved-release");
    fs::rename(&release, &moved).expect("move fixture release");
    symlink(&moved, &release).expect("symlink fixture");
    assert_eq!(
        remove_installation(&root, "dynamic", "release-1", "dynamic@release-1")
            .expect_err("symlink rejection")
            .code(),
        "installation-not-found"
    );
    assert!(moved.exists());
}
