#![cfg(target_os = "linux")]
//! Real forced-writer recovery in a fresh test-owned registry, without core dumps.
#[path = "support/pinned_child.rs"]
mod pinned_child;
#[path = "support/recovery_writer.rs"]
mod recovery_writer;
#[path = "support/runtime_seed.rs"]
mod runtime_seed;
use std::{
    fs,
    os::unix::process::ExitStatusExt,
    process::{Command, Stdio},
};
use zixcel_local_inference::RuntimeRegistry;
#[test]
fn recovery_after_forced_writer_termination_preserves_bytes_revision_and_original_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    if recovery_writer::child_if_requested()? {
        return Ok(());
    }
    let temporary = tempfile::tempdir()?;
    let (registry, definition) = runtime_seed::seed_runtime(temporary.path());
    let committed = registry.admit_runtime(definition, registry.snapshot()?.revision)?;
    let before = registry.snapshot()?;
    let root = temporary.path().join("registry");
    let parent_dumpable = rustix::process::dumpable_behavior()?;
    let mut command = Command::new(std::env::current_exe()?);
    command.args(["--exact", "recovery_after_forced_writer_termination_preserves_bytes_revision_and_original_receipt"])
        .env_clear().env("ZIXCEL_TEST_CRASH_REGISTRY", &root)
        .env("ZIXCEL_TEST_CRASH_PARENT", std::process::id().to_string())
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut writer = pinned_child::PinnedChild::spawn(command)?;
    recovery_writer::wait_ready(&mut writer, &root);
    let status = writer.interrupt()?;
    assert_eq!(status.signal(), Some(9));
    assert!(!status.core_dumped());
    assert_eq!(writer.finish()?, status);
    assert_eq!(rustix::process::dumpable_behavior()?, parent_dumpable);
    let bytes = fs::read(root.join("admission.redb"))?;
    let error = RuntimeRegistry::open_existing(&root)
        .expect_err("interrupted writer requires explicit recovery");
    assert_eq!(error.code(), "registry-recovery-required");
    assert_eq!(
        fs::read(root.join("admission.redb"))?,
        bytes,
        "failed read cannot repair state"
    );
    let restored = RuntimeRegistry::recover(&root)?;
    assert_eq!(restored.snapshot()?.revision, before.revision);
    assert_eq!(restored.runtime(&committed.0.runtime_ref)?, committed.0);
    assert_eq!(
        restored.admit_runtime(
            committed.0.definition.clone(),
            committed.1.previous_revision.clone()
        )?,
        committed
    );
    let absent = temporary.path().join("missing");
    assert!(RuntimeRegistry::recover(&absent).is_err());
    assert!(!absent.exists());
    Ok(())
}
