use std::{
    io::{self, Write},
    path::Path,
    process::{Command, ExitStatus, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};
pub static FORCED_CLEANUPS: AtomicUsize = AtomicUsize::new(0);
pub struct Worker(super::pinned_child::PinnedChild);
impl Worker {
    pub fn spawn(mode: &str, receipt: &Path) -> Self {
        let mut command = Command::new(std::env::current_exe().expect("test executable"));
        command
            .args(["--exact", "worker_entry"])
            .env_clear()
            .env("BOUNDARY_TEST_MODE", mode)
            .env("BOUNDARY_TEST_PARENT", std::process::id().to_string())
            .env("BOUNDARY_TEST_RECEIPT", receipt)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        Self(super::pinned_child::PinnedChild::spawn(command).expect("pidfd-owned child"))
    }
    pub fn still_running(&mut self) -> bool {
        self.0.running().expect("observe own child")
    }
    pub fn finish(&mut self) -> io::Result<ExitStatus> {
        self.0.finish()
    }
    pub fn interrupt(&mut self) -> io::Result<ExitStatus> {
        self.0.interrupt()
    }
    pub fn release_abort(&mut self) {
        self.0
            .stdin()
            .expect("input")
            .write_all(b"!")
            .expect("release verified fault");
    }
    pub fn request_ready(&mut self, receipt: &Path) {
        self.0
            .stdin()
            .expect("input")
            .write_all(b"?")
            .expect("ready request");
        let deadline = Instant::now() + Duration::from_secs(3);
        while std::fs::read(receipt).ok().as_deref() != Some(b"attached:dumpable=0") {
            assert!(
                self.still_running(),
                "worker stopped before verified protection"
            );
            assert!(Instant::now() < deadline, "readiness bound exceeded");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        if self.finish().is_err() {
            // Failure cleanup targets only this live Child handle, never a process group.
            FORCED_CLEANUPS.fetch_add(1, Ordering::SeqCst);
            let _ = self.0.interrupt();
        }
    }
}
