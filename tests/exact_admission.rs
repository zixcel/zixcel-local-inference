use std::fs;
#[path = "support/pinned_child.rs"]
mod pinned_child;
use std::process::{Command, Stdio};
use std::time::Duration;
use zixcel_local_inference::{ArtifactFormat, RuntimeAdmission};
use zixcel_local_inference::{ModelConfiguration, RuntimeConfiguration, RuntimeRegistry};

#[path = "support/runtime_seed.rs"]
mod runtime_seed;
use runtime_seed::{descriptor, gguf, seed_runtime, static_elf};

fn start_cli(root: &std::path::Path, request: &std::path::Path) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_zixcel-local-inference"))
        .args(["registry", "--state"])
        .arg(root)
        .arg("--request")
        .arg(request)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("fixture operation succeeds")
}

#[test]
fn killed_pending_process_cannot_admit_and_owner_lock_is_reusable() {
    let temporary = tempfile::tempdir().expect("isolated process fixture");
    let (registry, definition) = seed_runtime(temporary.path());
    let root = temporary.path().join("registry");
    let revision = registry.snapshot().expect("before crash").revision;
    let request = temporary.path().join("request.json");
    fs::write(
        &request,
        serde_json::to_vec(&zixcel_local_inference::RegistryCommand::AdmitRuntime {
            definition,
            expected_revision: revision.clone(),
        })
        .expect("command"),
    )
    .expect("request file");
    let lock = fs::File::open(root.join("admission.lock")).expect("existing owner lock");
    lock.lock().expect("hold publication boundary");
    let mut command = Command::new(env!("CARGO_BIN_EXE_zixcel-local-inference"));
    command
        .args(["registry", "--state"])
        .arg(&root)
        .arg("--request")
        .arg(&request)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = pinned_child::PinnedChild::spawn(command).expect("pin exact pending CLI");
    assert!(child.stdin().is_none(), "no inherited interactive input");
    assert!(child.running().expect("pending child exists"));
    // The child cannot pass admission while the parent holds this owner boundary.
    let interrupted = child.interrupt().expect("pidfd-only pending interruption");
    assert_eq!(child.finish().expect("cached exact exit"), interrupted);
    drop(lock);
    assert_eq!(
        registry
            .snapshot()
            .expect("after interrupted request")
            .revision,
        revision
    );
    let committed = start_cli(&root, &request)
        .wait_with_output()
        .expect("new explicit request");
    assert!(
        committed.status.success(),
        "{}",
        String::from_utf8_lossy(&committed.stderr)
    );
    let replay = start_cli(&root, &request)
        .wait_with_output()
        .expect("lost response replay");
    assert_eq!(committed.stdout, replay.stdout);
    assert_eq!(
        registry
            .snapshot()
            .expect("one admitted runtime")
            .runtimes
            .len(),
        1
    );
}

#[test]
fn runtime_process_races_replay_and_configuration_conflicts_preserve_exact_identity() {
    let temporary = tempfile::tempdir().expect("fixture operation succeeds");
    let (registry, definition) = seed_runtime(temporary.path());
    let root = temporary.path().join("registry");
    let expected = registry
        .snapshot()
        .expect("fixture operation succeeds")
        .revision;
    let request = temporary.path().join("request.json");
    let command = zixcel_local_inference::RegistryCommand::AdmitRuntime {
        definition: definition.clone(),
        expected_revision: expected.clone(),
    };
    fs::write(
        &request,
        serde_json::to_vec(&command).expect("fixture operation succeeds"),
    )
    .expect("fixture operation succeeds");
    let first = start_cli(&root, &request);
    let second = start_cli(&root, &request);
    let first = first
        .wait_with_output()
        .expect("fixture operation succeeds");
    let second = second
        .wait_with_output()
        .expect("fixture operation succeeds");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(
        registry
            .snapshot()
            .expect("fixture operation succeeds")
            .runtimes
            .len(),
        1
    );
    let mut changed = definition;
    changed.model_configuration.context_tokens = 1024;
    assert_eq!(
        registry
            .admit_runtime(changed.clone(), expected)
            .expect_err("invalid admission rejects")
            .code(),
        "admission-conflict"
    );
    let newer = registry
        .admit_runtime(
            changed,
            registry
                .snapshot()
                .expect("fixture operation succeeds")
                .revision,
        )
        .expect("fixture operation succeeds")
        .0;
    let reopened = RuntimeRegistry::open_existing(&root).expect("fixture operation succeeds");
    assert_eq!(
        reopened
            .snapshot()
            .expect("fixture operation succeeds")
            .runtimes
            .len(),
        2
    );
    assert_eq!(
        reopened
            .runtime(&newer.runtime_ref)
            .expect("fixture operation succeeds"),
        newer
    );
    assert_eq!(
        start_cli(&root, &request)
            .wait_with_output()
            .expect("fixture operation succeeds")
            .stdout,
        first.stdout
    );
    assert!(!root.join("processes").exists());
}

#[test]
fn wrong_runtime_references_platform_configuration_and_capacity_reject_before_commit() {
    let temporary = tempfile::tempdir().expect("fixture operation succeeds");
    let (registry, definition) = seed_runtime(temporary.path());
    let before = registry
        .snapshot()
        .expect("fixture operation succeeds")
        .revision;
    let mut bad = definition.clone();
    bad.model_architecture = "not-the-artifact-architecture".into();
    assert_eq!(
        registry
            .admit_runtime(bad, before.clone())
            .expect_err("invalid admission rejects")
            .code(),
        "unsupported-runtime"
    );
    let mut bad = definition.clone();
    bad.model_artifact_ref = "f".repeat(64);
    assert_eq!(
        registry
            .admit_runtime(bad, before.clone())
            .expect_err("invalid admission rejects")
            .code(),
        "artifact-not-found"
    );
    let mut bad = definition.clone();
    bad.runtime_configuration.threads = 0;
    assert_eq!(
        registry
            .admit_runtime(bad, before.clone())
            .expect_err("invalid admission rejects")
            .code(),
        "invalid-configuration"
    );
    assert_eq!(
        registry
            .snapshot()
            .expect("fixture operation succeeds")
            .revision,
        before
    );
    let root = temporary.path().join("registry");
    for index in 0..126 {
        fs::write(root.join("objects").join(format!("unfinished-{index}")), [])
            .expect("fixture operation succeeds");
    }
    let source = temporary.path().join("new-model");
    let bytes = gguf(2.0);
    fs::write(&source, &bytes).expect("fixture operation succeeds");
    assert_eq!(
        registry
            .admit_artifact(
                &source,
                descriptor(&bytes, ArtifactFormat::GgufV3),
                before.clone()
            )
            .expect_err("invalid admission rejects")
            .code(),
        "capacity-exceeded"
    );
    assert_eq!(
        registry
            .snapshot()
            .expect("fixture operation succeeds")
            .revision,
        before
    );
}

#[test]
fn independent_cli_processes_share_one_receipt_and_reads_are_identical() {
    let temporary = tempfile::tempdir().expect("fixture root");
    let root = temporary.path().join("registry");
    let registry = RuntimeRegistry::provision(&root).expect("registry");
    let source = temporary.path().join("source");
    let bytes = gguf(4.0);
    fs::write(&source, &bytes).expect("fixture");
    let request = zixcel_local_inference::RegistryCommand::AdmitArtifact {
        source: source.clone(),
        definition: descriptor(&bytes, ArtifactFormat::GgufV3),
        expected_revision: zixcel_revision::RevisionRef::default(),
    };
    let input = temporary.path().join("request.json");
    fs::write(&input, serde_json::to_vec(&request).expect("request")).expect("write request");
    let spawn = || {
        Command::new(env!("CARGO_BIN_EXE_zixcel-local-inference"))
            .args(["registry", "--state"])
            .arg(&root)
            .arg("--request")
            .arg(&input)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("independent CLI")
    };
    let first = spawn();
    let second = spawn();
    let a = first.wait_with_output().expect("first result");
    let b = second.wait_with_output().expect("second result");
    assert!(a.status.success(), "{}", String::from_utf8_lossy(&a.stderr));
    assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));
    assert_eq!(a.stdout, b.stdout, "exact original receipt");
    assert_eq!(registry.snapshot().expect("snapshot").artifacts.len(), 1);
    assert_eq!(registry.snapshot().expect("snapshot").revision.sequence, 1);
    fs::remove_file(source).expect("delivery removed after successful admission");
    let retry = spawn().wait_with_output().expect("lost response retry");
    assert!(retry.status.success());
    assert_eq!(retry.stdout, a.stdout);
    let inspect = zixcel_local_inference::RegistryCommand::Inspect {};
    fs::write(&input, serde_json::to_vec(&inspect).expect("request")).expect("write request");
    let before = fs::read(root.join("admission.redb")).expect("DB image");
    let cli = spawn().wait_with_output().expect("CLI inspect");
    assert!(cli.status.success());
    let direct = zixcel_local_inference::manage_registry(&root, inspect).expect("owner read");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&cli.stdout).expect("JSON"),
        serde_json::to_value(direct).expect("owner JSON")
    );
    assert_eq!(
        before,
        fs::read(root.join("admission.redb")).expect("unchanged DB")
    );
}

#[test]
fn cancellation_timeout_and_linked_paths_preserve_empty_state() {
    let temporary = tempfile::tempdir().expect("fixture root");
    let root = temporary.path().join("registry");
    let registry = RuntimeRegistry::provision(&root).expect("registry");
    let source = temporary.path().join("source");
    let bytes = gguf(1.0);
    fs::write(&source, &bytes).expect("fixture");
    let control = zixcel_local_inference::VerificationControl::new(Duration::from_secs(30));
    control.cancel();
    let expired = zixcel_local_inference::VerificationControl::new(Duration::ZERO);
    for (control, expected) in [(&control, "cancelled"), (&expired, "timeout")] {
        assert_eq!(
            registry
                .admit_artifact_controlled(
                    &source,
                    descriptor(&bytes, ArtifactFormat::GgufV3),
                    zixcel_revision::RevisionRef::default(),
                    control
                )
                .expect_err("bounded stop")
                .code(),
            expected
        );
    }
    let link = temporary.path().join("linked");
    std::os::unix::fs::symlink(&source, &link).expect("link");
    assert_eq!(
        registry
            .admit_artifact(
                &link,
                descriptor(&bytes, ArtifactFormat::GgufV3),
                zixcel_revision::RevisionRef::default()
            )
            .expect_err("symlink rejected")
            .code(),
        "unsupported-artifact"
    );
    assert_eq!(registry.snapshot().expect("unchanged").revision.sequence, 0);
    assert_eq!(
        fs::read_dir(root.join("objects")).expect("objects").count(),
        0
    );
    let invalid = zixcel_local_inference::RegistryCommand::from_json(
        br#"{"operation":"inspect","selectBestModel":true}"#,
    );
    assert!(invalid.is_err(), "read cannot smuggle mutations");
}

#[test]
fn immutable_bytes_exact_runtime_restart_replay_and_tampering() {
    let temporary = tempfile::tempdir().expect("fixture root");
    let root = temporary.path().join("registry");
    let source = temporary.path().join("source.any-extension");
    let binary = temporary.path().join("runtime.input");
    let registry = RuntimeRegistry::provision(&root).expect("provision");
    let bytes = gguf(1.0);
    fs::write(&source, &bytes).expect("fixture");
    let definition = descriptor(&bytes, ArtifactFormat::GgufV3);
    let (model, receipt) = registry
        .admit_artifact(
            &source,
            definition.clone(),
            zixcel_revision::RevisionRef::default(),
        )
        .expect("model admission");
    assert_eq!(model.architecture, "test-fixture");
    let replay = registry
        .admit_artifact(&source, definition, zixcel_revision::RevisionRef::default())
        .expect("lost response replay");
    assert_eq!(replay, (model.clone(), receipt.clone()));
    let executable = static_elf();
    fs::write(&binary, &executable).expect("binary fixture");
    let (implementation, _) = registry
        .admit_artifact(
            &binary,
            descriptor(&executable, ArtifactFormat::StaticElf64),
            receipt.committed_revision,
        )
        .expect("implementation admission");
    let request = RuntimeAdmission {
        model_artifact_ref: model.artifact_ref.clone(),
        implementation_ref: implementation.artifact_ref,
        model_configuration: ModelConfiguration {
            context_tokens: 1024,
            batch_tokens: 64,
        },
        runtime_configuration: RuntimeConfiguration {
            threads: 1,
            memory_bytes: 1024 * 1024 * 1024,
        },
        engine: "format-fixture-not-an-inference-engine".into(),
        engine_revision: "test-1".into(),
        model_architecture: "test-fixture".into(),
    };
    let (runtime, receipt) = registry
        .admit_runtime(
            request.clone(),
            registry.snapshot().expect("snapshot").revision,
        )
        .expect("runtime definition");
    assert_eq!(
        registry
            .admit_runtime(request, zixcel_revision::RevisionRef::default())
            .expect("replay"),
        (runtime.clone(), receipt)
    );
    let changed = gguf(2.0);
    fs::write(&source, &changed).expect("same path replaced");
    registry
        .verify_artifact(&model.artifact_ref)
        .expect("managed original remains valid");
    let (new_model, _) = registry
        .admit_artifact(
            &source,
            descriptor(&changed, ArtifactFormat::GgufV3),
            registry.snapshot().expect("snapshot").revision,
        )
        .expect("new exact bytes");
    assert_ne!(new_model.artifact_ref, model.artifact_ref);
    drop(registry);
    let registry = RuntimeRegistry::open_existing(&root).expect("restart");
    assert_eq!(
        registry
            .runtime(&runtime.runtime_ref)
            .expect("exact runtime"),
        runtime
    );
    assert_eq!(registry.snapshot().expect("snapshot").artifacts.len(), 3);
    let database = fs::read(root.join("admission.redb")).expect("database before observation");
    registry
        .verify_runtime(&runtime.runtime_ref)
        .expect("current static envelope available");
    assert_eq!(
        fs::read(root.join("admission.redb")).expect("database after observation"),
        database
    );
    check_tampering(&registry, &root, &model, &runtime, &changed);
}

fn check_tampering(
    registry: &RuntimeRegistry,
    root: &std::path::Path,
    model: &zixcel_local_inference::AdmittedArtifact,
    runtime: &zixcel_local_inference::AdmittedRuntime,
    changed: &[u8],
) {
    let object = root.join("objects").join(&model.definition.sha256);
    fs::remove_file(&object).expect("simulate loss");
    assert_eq!(
        registry
            .verify_artifact(&model.artifact_ref)
            .expect_err("unavailable")
            .code(),
        "artifact-unavailable"
    );
    assert!(!object.exists(), "read must not repair");
    assert_eq!(
        registry
            .verify_runtime(&runtime.runtime_ref)
            .expect_err("runtime bytes unavailable")
            .code(),
        "artifact-unavailable"
    );
    fs::write(&object, changed).expect("simulate substitution");
    assert_eq!(
        registry
            .verify_artifact(&model.artifact_ref)
            .expect_err("corrupt")
            .code(),
        "artifact-corrupt"
    );
    assert_eq!(
        registry
            .runtime(&runtime.runtime_ref)
            .expect("definition preserved"),
        *runtime
    );
    assert_eq!(
        registry
            .verify_runtime(&runtime.runtime_ref)
            .expect_err("runtime bytes corrupt")
            .code(),
        "artifact-corrupt"
    );
}

#[test]
fn invalid_artifacts_and_stale_configuration_do_not_mutate_registry() {
    let temporary = tempfile::tempdir().expect("fixture root");
    let root = temporary.path().join("registry");
    let registry = RuntimeRegistry::provision(&root).expect("registry");
    let source = temporary.path().join("model.gguf");
    let valid = gguf(1.0);
    let mut oversized = valid.clone();
    oversized[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
    let cases = [b"not a model".to_vec(), valid[..40].to_vec(), oversized];
    for bytes in cases {
        fs::write(&source, &bytes).expect("fixture");
        assert!(
            registry
                .admit_artifact(
                    &source,
                    descriptor(&bytes, ArtifactFormat::GgufV3),
                    zixcel_revision::RevisionRef::default()
                )
                .is_err()
        );
        assert_eq!(registry.snapshot().expect("snapshot").revision.sequence, 0);
        assert_eq!(
            fs::read_dir(root.join("objects")).expect("objects").count(),
            0
        );
    }
    fs::write(&source, &valid).expect("fixture");
    let mut invalid_digest = descriptor(&valid, ArtifactFormat::GgufV3);
    invalid_digest.sha256 = "0".repeat(64);
    assert_eq!(
        registry
            .admit_artifact(
                &source,
                invalid_digest,
                zixcel_revision::RevisionRef::default()
            )
            .expect_err("digest")
            .code(),
        "digest-mismatch"
    );
    let (_, receipt) = registry
        .admit_artifact(
            &source,
            descriptor(&valid, ArtifactFormat::GgufV3),
            zixcel_revision::RevisionRef::default(),
        )
        .expect("valid");
    let changed = gguf(3.0);
    fs::write(&source, &changed).expect("changed");
    assert_eq!(
        registry
            .admit_artifact(
                &source,
                descriptor(&changed, ArtifactFormat::GgufV3),
                zixcel_revision::RevisionRef::default()
            )
            .expect_err("stale")
            .code(),
        "admission-conflict"
    );
    assert_eq!(
        registry.snapshot().expect("unchanged").revision,
        receipt.committed_revision
    );
    assert_eq!(
        fs::read_dir(root.join("objects")).expect("objects").count(),
        1
    );
}

#[test]
fn exact_registry_is_explicit_and_configuration_identity_is_closed() {
    let root = tempfile::tempdir().expect("fixture");
    let state = root.path().join("exact");
    assert_eq!(
        RuntimeRegistry::open_existing(&state)
            .expect_err("missing")
            .code(),
        "registry-missing"
    );
    assert!(!state.exists());
    let registry = RuntimeRegistry::provision(&state).expect("explicit provision");
    assert!(registry.snapshot().expect("read").runtimes.is_empty());
    let first = ModelConfiguration::from_json(br#"{"contextTokens":1024,"batchTokens":128}"#)
        .expect("config");
    let reordered = ModelConfiguration::from_json(br#"{"batchTokens":128,"contextTokens":1024}"#)
        .expect("config");
    assert_eq!(
        first.identity().expect("identity"),
        reordered.identity().expect("identity")
    );
    let other = ModelConfiguration::from_json(br#"{"contextTokens":2048,"batchTokens":128}"#)
        .expect("config");
    assert_ne!(
        first.identity().expect("identity"),
        other.identity().expect("identity")
    );
    for invalid in [
        br#"{"contextTokens":0,"batchTokens":128}"#.as_slice(),
        br#"{"contextTokens":1024,"batchTokens":128,"prompt":"no"}"#.as_slice(),
        br#"{"contextTokens":1024,"contextTokens":1024,"batchTokens":128}"#.as_slice(),
    ] {
        assert_eq!(
            ModelConfiguration::from_json(invalid)
                .expect_err("reject")
                .code(),
            "invalid-configuration"
        );
    }
    assert!(RuntimeConfiguration::from_json(br#"{"threads":0,"memoryBytes":1073741824}"#).is_err());
    assert_eq!(registry.snapshot().expect("unchanged").revision.sequence, 0);
    drop(registry);
    assert!(
        RuntimeRegistry::open_existing(&state)
            .expect("restart")
            .snapshot()
            .expect("read")
            .artifacts
            .is_empty()
    );
}
