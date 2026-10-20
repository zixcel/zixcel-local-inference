use std::{
    fs,
    io::{Read, Write},
    path::Path,
};
use zixcel_local_inference::backend_process::{BackendProcessContext, WorkerLimits};
pub fn child_if_requested() -> Result<bool, Box<dyn std::error::Error>> {
    let Some(path) = std::env::var_os("ZIXCEL_TEST_CRASH_REGISTRY") else {
        return Ok(false);
    };
    let root = std::path::PathBuf::from(path);
    let parent = std::env::var("ZIXCEL_TEST_CRASH_PARENT")?.parse()?;
    BackendProcessContext::attach(
        parent,
        WorkerLimits {
            address_space_bytes: 512 * 1024 * 1024,
            cpu_seconds: 5,
        },
    )?
    .check()?;
    let backend = zixcel_revision::RedbBackend::recover_existing(root.join("admission.redb"))?;
    let mut byte = [0];
    if std::io::stdin().read(&mut byte)? != 1 || byte[0] != b'?' {
        return Ok(true);
    }
    assert_eq!(
        rustix::process::dumpable_behavior()?,
        rustix::process::DumpableBehavior::NotDumpable
    );
    fs::write(root.join("test-writer-ready"), b"non-dumpable-writer")?;
    let _ = std::io::stdin().read(&mut byte)?;
    drop(backend);
    Ok(true)
}
pub fn wait_ready(child: &mut super::pinned_child::PinnedChild, root: &Path) {
    child
        .stdin()
        .expect("writer input")
        .write_all(b"?")
        .expect("request writer readback");
    let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if fs::read(root.join("test-writer-ready")).ok().as_deref() == Some(b"non-dumpable-writer")
        {
            return;
        }
        assert!(
            child.running().expect("observe exact writer"),
            "writer ended before readiness"
        );
        assert!(std::time::Instant::now() < end, "writer readiness deadline");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}
