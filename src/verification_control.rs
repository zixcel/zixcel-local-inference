//! Caller cancellation and a bounded cooperative deadline; no task or process ownership.
use crate::LocalInferenceError;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub struct VerificationControl {
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
}
impl VerificationControl {
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self {
            deadline: Instant::now() + timeout.min(Duration::from_secs(30)),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
    pub(crate) fn with_cancellation(timeout: Duration, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            deadline: Instant::now() + timeout.min(Duration::from_secs(30)),
            cancelled,
        }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    /// Check the caller-owned observation deadline and cancellation without mutation.
    pub fn check(&self) -> Result<(), LocalInferenceError> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(LocalInferenceError::new("cancelled"));
        }
        if Instant::now() >= self.deadline {
            return Err(LocalInferenceError::new("timeout"));
        }
        Ok(())
    }
}
