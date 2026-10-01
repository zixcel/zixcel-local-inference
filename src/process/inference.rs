//! Bounded physical requests inside the existing process owner. No semantic
//! store, process identity mint, queue, routing or automatic replay/restart.
//! Hatter ecosystem 2026: bind finite execution time once in this owner's clock.
use super::{Live, ProcessSnapshot, ProcessState, snapshot};
use crate::inference::{Command, InferenceError as Error, InferenceRequest, MAX_REQUESTS, Reply};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Private, attempt-specific clock. Never serialized as a portable timestamp.
/// Created only after validation/capacity checks, never by receipt/replay paths.
#[derive(Clone, Copy)]
pub(super) struct ExecutionClock {
    pub(super) started: Instant,
    pub(super) deadline: Instant,
    pub(super) accepted_at_epoch_ms: Option<u64>,
}
impl ExecutionClock {
    fn accept(budget_ms: u32) -> Result<Self, Error> {
        Self::at(budget_ms, Instant::now(), SystemTime::now())
    }
    fn at(budget_ms: u32, started: Instant, observed_at: SystemTime) -> Result<Self, Error> {
        if budget_ms == 0 || budget_ms > crate::inference::MAX_EXECUTION_BUDGET_MS {
            return Err(Error::InvalidExecutionBudget);
        }
        Ok(Self {
            started,
            deadline: started + Duration::from_millis(u64::from(budget_ms)),
            accepted_at_epoch_ms: observed_at
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|d| u64::try_from(d.as_millis()).ok()),
        })
    }
    pub(super) fn check(self) -> Result<(), Error> {
        if Instant::now() >= self.deadline {
            Err(Error::ExecutionTimedOut)
        } else {
            Ok(())
        }
    }
}

#[derive(Default)]
pub(super) struct Requests {
    entries: BTreeMap<String, Entry>,
}
struct Entry {
    process_ref: String,
    state: Arc<Mutex<Reply>>,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Entry {
    fn observe(&mut self) -> Result<Reply, Error> {
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(worker) = self.worker.take()
        {
            let failed = worker.join().is_err();
            let mut state = self.state.lock().map_err(|_| Error::RuntimeUnavailable)?;
            if failed || matches!(*state, Reply::Running) {
                *state = Reply::Failed(Error::RuntimeUnavailable);
            }
        }
        Ok(self
            .state
            .lock()
            .map_err(|_| Error::RuntimeUnavailable)?
            .clone())
    }
}
impl Drop for Entry {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
pub(super) fn current(
    value: &Arc<Mutex<ProcessSnapshot>>,
    request: &InferenceRequest,
) -> Result<(), Error> {
    let state = snapshot(value).map_err(|_| Error::RuntimeUnavailable)?;
    if state.process_ref != request.process_ref {
        return Err(Error::StaleRuntimeProcess);
    }
    if state.runtime_ref != request.runtime_ref {
        return Err(Error::RuntimeProcessMismatch);
    }
    match state.state {
        ProcessState::Ready => {}
        ProcessState::Loading => return Err(Error::RuntimeNotReady),
        _ => return Err(Error::RuntimeUnavailable),
    }
    Ok(())
}
impl Requests {
    pub(super) fn command(&mut self, live: Option<&Live>, command: Command) -> Reply {
        match self.handle(live, command) {
            Ok(reply) => reply,
            Err(e) => Reply::Failed(e),
        }
    }
    fn handle(&mut self, live: Option<&Live>, command: Command) -> Result<Reply, Error> {
        let cancel = matches!(command, Command::InferenceCancel { .. });
        match command {
            Command::Receipt { reference, cancel } => {
                reference.validate()?;
                let entry = self
                    .entries
                    .get_mut(&reference.request_ref)
                    .ok_or(Error::UnknownRequest)?;
                if entry.process_ref != reference.process_ref {
                    return Err(Error::StaleRuntimeProcess);
                }
                if cancel {
                    entry.cancel.store(true, Ordering::Release);
                }
                entry.observe()
            }
            Command::InferenceInspect { request_ref }
            | Command::InferenceCancel { request_ref } => {
                let entry = self
                    .entries
                    .get_mut(&request_ref)
                    .ok_or(Error::UnknownRequest)?;
                if cancel {
                    entry.cancel.store(true, Ordering::Release);
                }
                entry.observe()
            }
            Command::Infer { request } => {
                // A request reference covers all exact body fields; an altered
                // body cannot be confused with a cached execution.
                request.validate()?;
                if let Some(entry) = self.entries.get_mut(&request.request_ref) {
                    return entry.observe();
                }
                if self.entries.len() >= MAX_REQUESTS {
                    return Err(Error::CapacityExceeded);
                }
                for entry in self.entries.values_mut() {
                    if matches!(entry.observe()?, Reply::Running) {
                        return Err(Error::CapacityExceeded);
                    }
                }
                let live = live.ok_or(Error::RuntimeUnavailable)?;
                current(&live.value, &request)?;
                let endpoint = live
                    .endpoint
                    .lock()
                    .map_err(|_| Error::RuntimeUnavailable)?
                    .clone()
                    .ok_or(Error::RuntimeNotReady)?;
                let state = Arc::new(Mutex::new(Reply::Running));
                let cancel = Arc::new(AtomicBool::new(false));
                let value = live.value.clone();
                let result_state = state.clone();
                let request_cancel = cancel.clone();
                let request_ref = request.request_ref.clone();
                let process_ref = request.process_ref.clone();
                // One origin includes thread startup and conversion. Passing
                // this value into the worker must not start a second budget.
                let clock = ExecutionClock::accept(request.options.execution_budget_ms)?;
                let worker = std::thread::Builder::new()
                    .name("zixcel-inference".into())
                    .spawn(move || {
                        let result = super::launch::infer(
                            &endpoint,
                            &value,
                            &request,
                            &request_cancel,
                            clock,
                        );
                        if let Ok(mut state) = result_state.lock() {
                            *state = if request_cancel.load(Ordering::Acquire) {
                                Reply::Failed(Error::Cancelled)
                            } else {
                                match result {
                                    Ok(result) => Reply::Complete(Box::new(result)),
                                    Err(e) => Reply::Failed(e),
                                }
                            };
                        }
                    })
                    .map_err(|_| Error::RuntimeUnavailable)?;
                self.entries.insert(
                    request_ref,
                    Entry {
                        process_ref,
                        state,
                        cancel,
                        worker: Some(worker),
                    },
                );
                Ok(Reply::Running)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Read,
        net::TcpListener,
        time::{Duration, Instant},
    };

    #[test]
    fn utc_skew_and_adjustment_do_not_change_owner_monotonic_deadline() {
        let started = Instant::now();
        let creator_ms = 1_789_198_144_170_u64;
        // Exact historical 272ms relation, large forward/backward skew, and
        // a pre-epoch unavailable observation. No real system clock changes.
        for owner_ms in [
            creator_ms - 272,
            creator_ms + 86_400_000,
            creator_ms - 86_400_000,
        ] {
            let clock = ExecutionClock::at(
                30_000,
                started,
                UNIX_EPOCH + Duration::from_millis(owner_ms),
            )
            .expect("clock");
            assert_eq!(clock.deadline, started + Duration::from_secs(30));
            assert_eq!(clock.accepted_at_epoch_ms, Some(owner_ms));
            assert_eq!(clock.check(), Ok(()));
        }
        let unavailable = ExecutionClock::at(30_000, started, UNIX_EPOCH - Duration::from_secs(1))
            .expect("no UTC dependency");
        assert_eq!(unavailable.accepted_at_epoch_ms, None);
        assert_eq!(unavailable.deadline, started + Duration::from_secs(30));
        // Jump either direction after the original acceptance: an expired
        // monotonic attempt stays expired regardless of provenance observation.
        let old_start = started
            .checked_sub(Duration::from_secs(31))
            .expect("test clock supports an expired execution origin");
        for observation in [UNIX_EPOCH, UNIX_EPOCH + Duration::from_secs(9_999_999_999)] {
            let clock = ExecutionClock::at(30_000, old_start, observation).expect("clock");
            assert_eq!(clock.check(), Err(Error::ExecutionTimedOut));
        }
        for invalid in [0, 30_001, u32::MAX] {
            assert!(matches!(
                ExecutionClock::accept(invalid),
                Err(Error::InvalidExecutionBudget)
            ));
        }
    }

    #[test]
    fn physical_budget_starts_once_and_expired_receipt_cannot_renew_or_resend() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listen");
        listener.set_nonblocking(true).expect("mode");
        let live = live(listener.local_addr().expect("address").port());
        let mut owner = Requests::default();
        let request = crate::inference::specimen_budget(&"b".repeat(64), "one-attempt", 500);
        // Preparation/queue residence does not spend a physical owner's budget.
        std::thread::sleep(Duration::from_millis(510));
        let command = Command::Infer {
            request: Box::new(request.clone()),
        };
        assert_eq!(owner.command(Some(&live), command.clone()), Reply::Running);
        assert_eq!(owner.entries.len(), 1);
        let limit = Instant::now() + Duration::from_secs(2);
        let mut socket = loop {
            if let Ok((socket, _)) = listener.accept() {
                break socket;
            }
            assert!(Instant::now() < limit, "physical backend call");
            std::thread::sleep(Duration::from_millis(1));
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("bound");
        let mut bytes = [0; 8192];
        assert!(socket.read(&mut bytes).expect("input") > 0);
        // Re-presenting the exact request is an observation, not new time.
        assert_eq!(owner.command(Some(&live), command.clone()), Reply::Running);
        assert_eq!(
            finish(&mut owner, &live, request.request_ref()),
            Reply::Failed(Error::ExecutionTimedOut)
        );
        assert_eq!(
            owner.command(Some(&live), command),
            Reply::Failed(Error::ExecutionTimedOut)
        );
        assert_eq!(owner.entries.len(), 1);
        assert_eq!(socket.read(&mut bytes).expect("closed exchange"), 0);
        assert_eq!(
            listener
                .accept()
                .expect_err("exactly one physical effect")
                .kind(),
            std::io::ErrorKind::WouldBlock
        );
        let reference = request.reference();
        assert_eq!(
            owner.command(
                None,
                Command::Receipt {
                    reference: reference.clone(),
                    cancel: false
                }
            ),
            Reply::Failed(Error::ExecutionTimedOut)
        );
        // New owner has no receipt; a receipt lookup must not become execution.
        let mut restarted = Requests::default();
        assert_eq!(
            restarted.command(
                None,
                Command::Receipt {
                    reference,
                    cancel: false
                }
            ),
            Reply::Failed(Error::UnknownRequest)
        );
        assert!(restarted.entries.is_empty());
    }

    #[test]
    fn receipt_only_lookup_and_cancel_pin_incarnation_without_input_or_execution() {
        let reference = crate::inference::RequestReference {
            request_ref: "a".repeat(64),
            process_ref: "b".repeat(64),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(Reply::Running));
        let mut owner = Requests::default();
        owner.entries.insert(
            reference.request_ref.clone(),
            Entry {
                process_ref: reference.process_ref.clone(),
                state: state.clone(),
                cancel: cancel.clone(),
                worker: None,
            },
        );
        let mut wrong = reference.clone();
        wrong.process_ref = "c".repeat(64);
        assert_eq!(
            owner.command(
                None,
                Command::Receipt {
                    reference: wrong,
                    cancel: true
                }
            ),
            Reply::Failed(Error::StaleRuntimeProcess)
        );
        assert!(!cancel.load(Ordering::Acquire));
        assert_eq!(
            owner.command(
                None,
                Command::Receipt {
                    reference: reference.clone(),
                    cancel: false
                }
            ),
            Reply::Running
        );
        assert!(!cancel.load(Ordering::Acquire));
        assert_eq!(
            owner.command(
                None,
                Command::Receipt {
                    reference: reference.clone(),
                    cancel: true
                }
            ),
            Reply::Running
        );
        assert!(cancel.load(Ordering::Acquire));
        *state.lock().expect("state") = Reply::Failed(Error::Cancelled);
        assert_eq!(
            owner.command(
                None,
                Command::Receipt {
                    reference: reference.clone(),
                    cancel: false
                }
            ),
            Reply::Failed(Error::Cancelled)
        );
        owner.entries.clear();
        assert_eq!(
            owner.command(
                None,
                Command::Receipt {
                    reference,
                    cancel: false
                }
            ),
            Reply::Failed(Error::UnknownRequest)
        );
        assert!(owner.entries.is_empty());
    }
    fn live(port: u16) -> Live {
        Live {
            value: Arc::new(Mutex::new(ProcessSnapshot {
                runtime_ref: "a".repeat(64),
                process_ref: "b".repeat(64),
                state: ProcessState::Ready,
                reason: None,
                termination: None,
                rss_bytes: 0,
                peak_rss_bytes: 0,
                threads: 0,
                loaded_artifact_refs: vec![],
                observed_host_libraries: vec![],
            })),
            cancel: Arc::new(AtomicBool::new(false)),
            worker: None,
            endpoint: Arc::new(Mutex::new(Some(super::super::launch::Endpoint::specimen(
                port,
            )))),
        }
    }
    fn finish(owner: &mut Requests, live: &Live, reference: &str) -> Reply {
        let end = Instant::now() + Duration::from_secs(2);
        loop {
            let reply = owner.command(
                Some(live),
                Command::InferenceInspect {
                    request_ref: reference.into(),
                },
            );
            if !matches!(reply, Reply::Running) {
                return reply;
            }
            assert!(Instant::now() < end, "request failed to terminate");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn stale_or_mismatched_process_never_contacts_backend_and_unknown_cancel_is_local() {
        let mut exited = Entry {
            process_ref: "b".repeat(64),
            state: Arc::new(Mutex::new(Reply::Running)),
            cancel: Arc::new(AtomicBool::new(false)),
            worker: Some(std::thread::spawn(|| {})),
        };
        let end = Instant::now() + Duration::from_secs(1);
        while !exited.worker.as_ref().expect("worker").is_finished() {
            assert!(Instant::now() < end);
            std::thread::yield_now();
        }
        assert_eq!(
            exited.observe().expect("reaped"),
            Reply::Failed(Error::RuntimeUnavailable)
        );
        assert!(exited.worker.is_none());
        let listener = TcpListener::bind("127.0.0.1:0").expect("listen");
        listener.set_nonblocking(true).expect("mode");
        let live = live(listener.local_addr().expect("address").port());
        let mut owner = Requests::default();
        let stale = crate::inference::specimen(&"c".repeat(64));
        assert_eq!(
            owner.command(
                Some(&live),
                Command::Infer {
                    request: Box::new(stale)
                }
            ),
            Reply::Failed(Error::StaleRuntimeProcess)
        );
        let valid = crate::inference::specimen(&"b".repeat(64));
        live.value.lock().expect("state").runtime_ref = "c".repeat(64);
        assert_eq!(
            owner.command(
                Some(&live),
                Command::Infer {
                    request: Box::new(valid)
                }
            ),
            Reply::Failed(Error::RuntimeProcessMismatch)
        );
        assert_eq!(
            owner.command(
                Some(&live),
                Command::InferenceCancel {
                    request_ref: "unknown".into()
                }
            ),
            Reply::Failed(Error::UnknownRequest)
        );
        assert_eq!(
            listener.accept().expect_err("zero backend calls").kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert!(!live.cancel.load(Ordering::Acquire));
        assert!(owner.entries.is_empty());
    }
    #[test]
    fn mid_request_cancel_process_death_and_capacity_bound_preserve_exact_association() {
        for death in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").expect("listen");
            let port = listener.local_addr().expect("address").port();
            let live = live(port);
            let (tx, rx) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                listener.set_nonblocking(true).expect("mode");
                let end = Instant::now() + Duration::from_secs(3);
                let mut stream = loop {
                    if let Ok((stream, _)) = listener.accept() {
                        break stream;
                    }
                    assert!(Instant::now() < end);
                    std::thread::sleep(Duration::from_millis(5));
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .expect("timeout");
                let mut buf = [0; 8192];
                assert!(stream.read(&mut buf).expect("request") > 0);
                tx.send(()).expect("notify");
                // Peer must close this request socket when cancelled/retired.
                assert_eq!(stream.read(&mut buf).expect("bounded close"), 0);
            });
            let mut owner = Requests::default();
            let request = crate::inference::specimen_named(&"b".repeat(64), "A");
            let reference = request.request_ref.clone();
            assert_eq!(
                owner.command(
                    Some(&live),
                    Command::Infer {
                        request: Box::new(request)
                    }
                ),
                Reply::Running
            );
            rx.recv_timeout(Duration::from_secs(2))
                .expect("backend dispatch");
            let other = crate::inference::specimen_named(&"b".repeat(64), "B");
            assert_eq!(
                owner.command(
                    Some(&live),
                    Command::Infer {
                        request: Box::new(other)
                    }
                ),
                Reply::Failed(Error::CapacityExceeded)
            );
            if death {
                live.value.lock().expect("state").state = ProcessState::Failed;
            } else {
                assert_eq!(
                    owner.command(
                        Some(&live),
                        Command::InferenceCancel {
                            request_ref: reference.clone()
                        }
                    ),
                    Reply::Running
                );
            }
            // Replace the retired G1 owner view while its request still owns
            // the original read/transport lease. Never relabel the result G2.
            let replacement = self::live(port);
            replacement.value.lock().expect("G2").process_ref = "c".repeat(64);
            assert_eq!(
                finish(
                    &mut owner,
                    if death { &replacement } else { &live },
                    &reference
                ),
                Reply::Failed(if death {
                    Error::RuntimeUnavailable
                } else {
                    Error::Cancelled
                })
            );
            assert!(!live.cancel.load(Ordering::Acquire));
            assert_eq!(
                replacement.value.lock().expect("G2").state,
                ProcessState::Ready
            );
            worker.join().expect("backend");
        }
    }
}
