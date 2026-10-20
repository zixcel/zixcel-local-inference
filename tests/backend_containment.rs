#![cfg(target_os = "linux")]
//! Child-only interruption with dump protection; no model or memory exhaustion.
#[path = "support/pinned_child.rs"]
mod pinned_child;
#[path = "backend_containment/support.rs"]
mod support;
use std::{fs, io::Read, sync::atomic::Ordering};
use support::{FORCED_CLEANUPS, Worker};
use zixcel_local_inference::backend_process::{BackendProcessContext, WorkerLimits};
#[test]
fn worker_entry() -> Result<(), Box<dyn std::error::Error>> {
    let Ok(mode) = std::env::var("BOUNDARY_TEST_MODE") else {
        return Ok(());
    };
    let parent = std::env::var("BOUNDARY_TEST_PARENT")?.parse()?;
    let context = BackendProcessContext::attach(
        parent,
        WorkerLimits {
            address_space_bytes: 256 * 1024 * 1024,
            cpu_seconds: 5,
        },
    );
    if mode == "shared" {
        assert!(context.is_err());
        return Ok(());
    }
    context?.check()?;
    assert_eq!(
        rustix::process::dumpable_behavior()?,
        rustix::process::DumpableBehavior::NotDumpable
    );
    assert_eq!(
        rustix::process::getrlimit(rustix::process::Resource::Core).maximum,
        Some(0)
    );
    assert_eq!(
        rustix::process::getrlimit(rustix::process::Resource::As).maximum,
        Some(256 * 1024 * 1024)
    );
    if matches!(mode.as_str(), "sibling" | "forced" | "protected-abort") {
        let mut byte = [0];
        let _ = std::io::stdin().read(&mut byte)?;
        fs::write(
            std::env::var("BOUNDARY_TEST_RECEIPT")?,
            b"attached:dumpable=0",
        )?;
        let count = std::io::stdin().read(&mut byte)?;
        if mode == "protected-abort" && count == 1 && byte[0] == b'!' {
            BackendProcessContext::attach(
                parent,
                WorkerLimits {
                    address_space_bytes: 256 * 1024 * 1024,
                    cpu_seconds: 5,
                },
            )?
            .check()?;
            std::process::abort();
        }
        return Ok(());
    }
    fs::write(
        std::env::var("BOUNDARY_TEST_RECEIPT")?,
        b"attached:dumpable=0",
    )?;
    match mode.as_str() {
        "nonzero" => std::process::exit(23),
        "normal" => Ok(()),
        _ => Err("unsupported test mode".into()),
    }
}
#[test]
fn shared_group_rejects_before_native_loading_and_leaves_parent_limits_unchanged() {
    let directory = tempfile::tempdir().expect("private receipt");
    let original = rustix::process::getrlimit(rustix::process::Resource::As);
    let receipt = directory.path().join("unexpected");
    let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "worker_entry"])
        .env_clear()
        .env("BOUNDARY_TEST_MODE", "shared")
        .env("BOUNDARY_TEST_PARENT", std::process::id().to_string())
        .env("BOUNDARY_TEST_RECEIPT", &receipt)
        .spawn()
        .expect("non-isolated rejection child");
    assert!(child.wait().expect("natural exit").success());
    assert!(!receipt.exists());
    assert_eq!(
        rustix::process::getrlimit(rustix::process::Resource::As),
        original
    );
    assert_eq!(FORCED_CLEANUPS.load(Ordering::SeqCst), 0);
}
#[test]
fn natural_zero_and_nonzero_exits_leave_owner_and_unrelated_worker_running() {
    let directory = tempfile::tempdir().expect("private receipts");
    let original_pid = rustix::process::getpid();
    let original_group = rustix::process::getpgrp();
    let original_core = rustix::process::getrlimit(rustix::process::Resource::Core);
    let sibling_receipt = directory.path().join("sibling");
    let mut sibling = Worker::spawn("sibling", &sibling_receipt);
    sibling.request_ready(&sibling_receipt);
    for (mode, code) in [("normal", 0), ("nonzero", 23)] {
        let receipt = directory.path().join(mode);
        let mut child = Worker::spawn(mode, &receipt);
        let status = child.finish().expect("bounded natural completion");
        assert_eq!(status.code(), Some(code));
        assert!(receipt.exists());
        assert_eq!(rustix::process::getpid(), original_pid);
        assert_eq!(rustix::process::getpgrp(), original_group);
        assert_eq!(
            rustix::process::getrlimit(rustix::process::Resource::Core),
            original_core
        );
        assert!(sibling.still_running());
    }
    assert!(sibling.finish().expect("EOF completion").success());
    assert_eq!(FORCED_CLEANUPS.load(Ordering::SeqCst), 0);
}

#[path = "backend_containment/forced.rs"]
mod forced;
