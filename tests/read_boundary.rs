//! Missing-state reads must not provision catalog or endpoint-route storage.
use std::fs;
use std::process::Command;

#[test]
fn missing_reads_return_absence_without_creating_any_state() {
    let temporary = tempfile::tempdir().expect("test root");
    let commands: &[&[&str]] = &[
        &["source-list"],
        &["candidates", "--json"],
        &["inspect", "unknown"],
        &["source-request", "unknown"],
        &["route-list"],
    ];
    for (index, arguments) in commands.iter().enumerate() {
        let state = temporary.path().join(format!("missing-{index}"));
        let result = Command::new(env!("CARGO_BIN_EXE_zixcel-local-inference"))
            .args(*arguments)
            .arg("--state")
            .arg(&state)
            .output()
            .expect("CLI read");
        assert!(!state.exists(), "read created state: {arguments:?}");
        assert!(!result.status.success());
        let error: serde_json::Value = serde_json::from_slice(&result.stderr).expect("typed error");
        assert_eq!(error["reasonCode"], "registry-missing", "{arguments:?}");
    }
    assert_eq!(fs::read_dir(temporary.path()).expect("root").count(), 0);
}

#[test]
fn explicit_provision_does_not_admit_routes_or_select_a_model() {
    let temporary = tempfile::tempdir().expect("test root");
    let state = temporary.path().join("state");
    let binary = env!("CARGO_BIN_EXE_zixcel-local-inference");
    let provision = Command::new(binary)
        .args(["provision", "--state"])
        .arg(&state)
        .output()
        .expect("explicit provision");
    assert!(provision.status.success(), "{:?}", provision.stderr);
    for command in ["source-list", "route-list"] {
        let result = Command::new(binary)
            .args([command, "--state"])
            .arg(&state)
            .output()
            .expect("read provisioned registry");
        assert!(result.status.success());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&result.stdout).expect("JSON"),
            serde_json::json!([])
        );
    }
    fs::remove_dir(state.join("runtimes")).expect("simulate incomplete storage");
    let result = Command::new(binary)
        .args(["route-list", "--state"])
        .arg(&state)
        .output()
        .expect("read incomplete storage");
    assert!(!result.status.success());
    assert!(!state.join("runtimes").exists(), "read repaired storage");
}
