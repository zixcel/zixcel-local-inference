use super::{FORCED_CLEANUPS, Worker};
use std::{os::unix::process::ExitStatusExt, sync::atomic::Ordering};
#[test]
fn pinned_forced_termination_preserves_parent_and_unrelated_worker() {
    let root = tempfile::tempdir().expect("receipts");
    let original = rustix::process::dumpable_behavior().expect("parent dump policy");
    let mut sibling = Worker::spawn("sibling", &root.path().join("sibling"));
    sibling.request_ready(&root.path().join("sibling"));
    let mut target = Worker::spawn("forced", &root.path().join("target"));
    target.request_ready(&root.path().join("target"));
    let status = target.interrupt().expect("exact pidfd interrupt");
    assert_eq!(status.signal(), Some(9));
    assert!(!status.core_dumped());
    assert_eq!(target.interrupt().expect("cached status"), status);
    assert!(sibling.still_running());
    assert_eq!(
        rustix::process::dumpable_behavior().expect("unchanged parent"),
        original
    );
    assert!(
        sibling
            .finish()
            .expect("natural sibling completion")
            .success()
    );
    assert_eq!(FORCED_CLEANUPS.load(Ordering::SeqCst), 0);
}
#[test]
fn protected_abnormal_exit_does_not_dump_or_terminate_unrelated_worker() {
    let root = tempfile::tempdir().expect("receipts");
    let original = rustix::process::dumpable_behavior().expect("parent policy");
    let mut sibling = Worker::spawn("sibling", &root.path().join("sibling"));
    sibling.request_ready(&root.path().join("sibling"));
    let mut target = Worker::spawn("protected-abort", &root.path().join("target"));
    target.request_ready(&root.path().join("target"));
    target.release_abort();
    let status = target.finish().expect("bounded protected exit");
    assert_eq!(status.signal(), Some(6));
    assert!(!status.core_dumped());
    assert!(sibling.still_running());
    assert_eq!(
        rustix::process::dumpable_behavior().expect("unchanged parent"),
        original
    );
    assert!(sibling.finish().expect("sibling EOF").success());
    assert_eq!(FORCED_CLEANUPS.load(Ordering::SeqCst), 0);
}
