//! Proof of a dedicated Linux worker process, before any native backend is loaded.
//! This attaches an already spawned child; it does not spawn or signal its owner.
use crate::LocalInferenceError;
use rustix::process::{
    DumpableBehavior, Resource, Rlimit, dumpable_behavior, getpgrp, getpid, getppid, getrlimit,
    set_dumpable_behavior, setrlimit,
};

#[derive(Clone, Debug)]
pub struct BackendProcessContext {
    pid: u32,
    parent: u32,
}
impl BackendProcessContext {
    /// Attach only a dedicated process-group leader with the exact live parent.
    /// The spawning owner must enforce deadlines, memory limits and group cleanup.
    /// The child arms Crowsi parent-death protection before native initialization.
    /// # Errors
    /// Rejects a shared process group, invalid parent or lost owner.
    pub fn attach(expected_parent: u32, limits: WorkerLimits) -> Result<Self, LocalInferenceError> {
        let pid = getpid();
        if getpgrp() != pid
            || expected_parent == pid.as_raw_nonzero().get().unsigned_abs()
            || getppid().map(|p| p.as_raw_nonzero().get().unsigned_abs()) != Some(expected_parent)
        {
            return Err(LocalInferenceError::new(
                "backend-process-isolation-required",
            ));
        }
        limits.validate()?;
        crowsi_transport_foundation::process::bind_parent(expected_parent)
            .map_err(|_| LocalInferenceError::new("backend-owner-unavailable"))?;
        set_dumpable_behavior(DumpableBehavior::NotDumpable)
            .map_err(|_| LocalInferenceError::new("backend-dump-protection-unavailable"))?;
        limits.apply()?;
        let context = Self {
            pid: pid.as_raw_nonzero().get().unsigned_abs(),
            parent: expected_parent,
        };
        context.check()?;
        Ok(context)
    }
    /// Reject use after crossing a process boundary or losing the owning process.
    /// # Errors
    /// Returns isolation/owner rejection; never repairs process state.
    pub fn check(&self) -> Result<(), LocalInferenceError> {
        if dumpable_behavior().ok() != Some(DumpableBehavior::NotDumpable) {
            return Err(LocalInferenceError::new(
                "backend-dump-protection-unavailable",
            ));
        }
        if getpid().as_raw_nonzero().get().unsigned_abs() != self.pid
            || getpgrp() != getpid()
            || getppid().map(|p| p.as_raw_nonzero().get().unsigned_abs()) != Some(self.parent)
        {
            return Err(LocalInferenceError::new("backend-owner-unavailable"));
        }
        Ok(())
    }
}

/// Finite per-worker limits. Virtual address space is not an RSS measurement.
#[derive(Clone, Copy, Debug)]
pub struct WorkerLimits {
    pub address_space_bytes: u64,
    pub cpu_seconds: u32,
}
impl WorkerLimits {
    fn validate(self) -> Result<(), LocalInferenceError> {
        if !(64 * 1024 * 1024..=16 * 1024 * 1024 * 1024).contains(&self.address_space_bytes)
            || !(1..=300).contains(&self.cpu_seconds)
        {
            return Err(LocalInferenceError::new("backend-resource-limit-invalid"));
        }
        Ok(())
    }
    fn apply(self) -> Result<(), LocalInferenceError> {
        for (resource, selected) in [
            (Resource::Core, 0),
            (Resource::As, self.address_space_bytes),
            (Resource::Cpu, u64::from(self.cpu_seconds)),
        ] {
            let old = getrlimit(resource);
            setrlimit(resource, lowered(old, selected))
                .map_err(|_| LocalInferenceError::new("backend-resource-limit-unavailable"))?;
        }
        Ok(())
    }
}

fn lowered(old: Rlimit, selected: u64) -> Rlimit {
    let maximum = old.maximum.unwrap_or(selected).min(selected);
    Rlimit {
        current: Some(old.current.unwrap_or(maximum).min(maximum)),
        maximum: Some(maximum),
    }
}
#[cfg(test)]
#[path = "backend_process_tests.rs"]
mod tests;
