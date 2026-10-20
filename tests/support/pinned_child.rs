//! Fault injection pins an owned child, never an ambient PID or process group.
use rustix::{
    fd::{AsFd, OwnedFd},
    process::{
        Pid, PidfdFlags, Signal, WaitId, WaitIdOptions, pidfd_open, pidfd_send_signal, waitid,
    },
};
use std::{
    io,
    os::unix::process::CommandExt,
    process::{Child, Command, ExitStatus},
    time::{Duration, Instant},
};
pub struct PinnedChild {
    child: Child,
    descriptor: OwnedFd,
}
impl PinnedChild {
    pub fn spawn(mut command: Command) -> io::Result<Self> {
        // Check platform support without creating a worker or sending any signal.
        drop(pidfd_open(rustix::process::getpid(), PidfdFlags::empty())?);
        let mut child = command.process_group(0).spawn()?;
        let pid =
            Pid::from_raw(i32::try_from(child.id()).map_err(|_| io::ErrorKind::InvalidInput)?)
                .ok_or(io::ErrorKind::InvalidInput)?;
        let descriptor = match pidfd_open(pid, PidfdFlags::empty()) {
            Ok(descriptor) => descriptor,
            Err(error) => {
                // Still-unreaped Child owns this PID; contain failed setup as well.
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        };
        Ok(Self { child, descriptor })
    }
    pub fn stdin(&mut self) -> Option<&mut std::process::ChildStdin> {
        self.child.stdin.as_mut()
    }
    pub fn running(&mut self) -> io::Result<bool> {
        self.child.try_wait().map(|s| s.is_none())
    }
    pub fn finish(&mut self) -> io::Result<ExitStatus> {
        drop(self.child.stdin.take());
        self.wait()
    }
    pub fn interrupt(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.child.try_wait()? {
            return Ok(status);
        }
        // WNOWAIT proves parentage without reaping before the pidfd signal.
        waitid(
            WaitId::PidFd(self.descriptor.as_fd()),
            WaitIdOptions::EXITED | WaitIdOptions::NOWAIT | WaitIdOptions::NOHANG,
        )?;
        match pidfd_send_signal(&self.descriptor, Signal::KILL) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => (),
            Err(error) => return Err(error.into()),
        }
        self.wait()
    }
    fn wait(&mut self) -> io::Result<ExitStatus> {
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= end {
                return Err(io::ErrorKind::TimedOut.into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
impl Drop for PinnedChild {
    fn drop(&mut self) {
        let _ = self.interrupt();
    }
}
