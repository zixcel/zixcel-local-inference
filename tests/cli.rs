mod support;

use std::fs;
use std::process::{Command, Output};

use tempfile::tempdir;

use support::{MODEL_BYTES, catalog, model, write_signed};

#[test]
fn cli_completes_source_to_removal_with_exact_data_checks() {
    let binary = env!("CARGO_BIN_EXE_zixcel-local-inference");
    assert_eq!(
        String::from_utf8(success(binary, &["--version"]).stdout).expect("version output"),
        "zixcel-local-inference 0.10.0\n"
    );
    let temporary = tempdir().expect("temporary directory");
    let state = temporary.path().join("state");
    let root = temporary.path().join("models");
    let envelope = temporary.path().join("catalog.signed.json");
    let public_key_file = temporary.path().join("catalog.pub");
    let artifact = temporary.path().join("delivered.gguf");
    let (key_id, public_key) = write_signed(
        &envelope,
        &catalog(1, vec![model("cli-model", "approved")]),
        8,
    );
    fs::write(&public_key_file, public_key).expect("public key");
    fs::write(&artifact, MODEL_BYTES).expect("artifact");
    let state = state.to_str().expect("state path");
    let root = root.to_str().expect("model root");
    let envelope = envelope.to_str().expect("catalog path");
    let public_key_file = public_key_file.to_str().expect("key path");
    let artifact = artifact.to_str().expect("artifact path");

    success(binary, &["provision", "--state", state]);
    register_and_refresh(binary, state, envelope, public_key_file, &key_id);
    configure_runtime(binary, state);
    install_check_and_remove(binary, state, root, artifact);
}

fn register_and_refresh(
    binary: &str,
    state: &str,
    envelope: &str,
    public_key_file: &str,
    key_id: &str,
) {
    success(
        binary,
        &[
            "source-add-file",
            "primary",
            "--state",
            state,
            "--catalog",
            envelope,
            "--key-id",
            key_id,
            "--public-key-file",
            public_key_file,
        ],
    );
    success(binary, &["refresh", "primary", "--state", state]);
    let candidates = json_success(binary, &["candidates", "--state", state, "--json"]);
    assert_eq!(candidates["candidates"].as_array().expect("array").len(), 1);
    assert_eq!(candidates["candidates"][0]["id"], "cli-model");
}

fn configure_runtime(binary: &str, state: &str) {
    success(
        binary,
        &[
            "route-add",
            "cpu",
            "--state",
            state,
            "--engine",
            "llama.cpp",
            "--protocol",
            "openai-compatible",
            "--endpoint",
            "http://127.0.0.1:8080/v1",
            "--capabilities",
            "classification",
        ],
    );
}

fn install_check_and_remove(binary: &str, state: &str, root: &str, artifact: &str) {
    let plan = json_success(
        binary,
        &[
            "plan",
            "cli-model",
            "--state",
            state,
            "--root",
            root,
            "--runtime",
            "cpu",
        ],
    );
    assert_eq!(plan["transportOwner"], "crowsi");
    assert_eq!(plan["expectedBytes"], MODEL_BYTES.len() as u64);
    assert_eq!(plan["automaticExecution"], false);

    let installed = json_success(
        binary,
        &[
            "install",
            "cli-model",
            "--state",
            state,
            "--root",
            root,
            "--artifact-file",
            artifact,
            "--runtime",
            "cpu",
        ],
    );
    assert_eq!(installed["state"], "artifact-ready");
    assert_eq!(installed["inferenceReady"], false);
    let status = json_success(
        binary,
        &["status", "cli-model", "--state", state, "--root", root],
    );
    assert_eq!(status[0]["artifactSha256"], installed["artifactSha256"]);
    assert_eq!(
        json_success(binary, &["installed", "--state", state, "--root", root])
            .as_array()
            .expect("installed array")
            .len(),
        1
    );
    success(
        binary,
        &[
            "remove",
            "cli-model",
            "--root",
            root,
            "--release",
            "release-1",
            "--confirm",
            "cli-model@release-1",
        ],
    );
    assert!(
        json_success(binary, &["installed", "--state", state, "--root", root])
            .as_array()
            .expect("empty installed array")
            .is_empty()
    );
}

fn success(binary: &str, arguments: &[&str]) -> Output {
    let output = Command::new(binary)
        .args(arguments)
        .output()
        .expect("execute CLI");
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn json_success(binary: &str, arguments: &[&str]) -> serde_json::Value {
    let output = success(binary, arguments);
    serde_json::from_slice(&output.stdout).expect("CLI JSON")
}
