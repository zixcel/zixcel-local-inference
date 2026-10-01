//! Explicit, external owner service. Registry identity is not live process identity.
//! Bounded physical inference, no semantic admission, routing or persisted PID truth.
use crate::{LocalInferenceError, RuntimeRegistry};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};
mod inference;
mod launch;
mod service;
mod termination;
pub use launch::child;
pub(crate) use service::inference_request;
pub use service::{request, serve, supervise};
pub use termination::ProcessTermination;

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProcessCommand {
    Inspect {},
    Start {
        runtime_ref: String,
        request_ref: String,
    },
    Stop {
        process_ref: String,
    },
    Restart {
        process_ref: String,
        runtime_ref: String,
        request_ref: String,
    },
    Shutdown {},
}
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessState {
    Loading,
    Ready,
    Stopping,
    Stopped,
    Failed,
}
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessSnapshot {
    pub runtime_ref: String,
    pub process_ref: String,
    pub state: ProcessState,
    pub reason: Option<String>,
    pub termination: Option<ProcessTermination>,
    pub rss_bytes: u64,
    pub peak_rss_bytes: u64,
    pub threads: u32,
    pub loaded_artifact_refs: Vec<String>,
    pub observed_host_libraries: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessReply {
    pub process: Option<ProcessSnapshot>,
    /// Exact bounded owner rejection; transport success is not operation success.
    pub rejection: Option<String>,
}
fn error(reason: &'static str) -> LocalInferenceError {
    LocalInferenceError::new(reason)
}
fn snapshot(value: &Arc<Mutex<ProcessSnapshot>>) -> Result<ProcessSnapshot, LocalInferenceError> {
    value
        .lock()
        .map(|v| v.clone())
        .map_err(|_| error("process-state-unavailable"))
}
pub(crate) fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl ProcessCommand {
    pub(crate) fn validate(&self) -> Result<(), LocalInferenceError> {
        let refs: Vec<&str> = match self {
            Self::Inspect {} | Self::Shutdown {} => vec![],
            Self::Stop { process_ref } => vec![process_ref],
            Self::Start {
                runtime_ref,
                request_ref,
            } => vec![runtime_ref, request_ref],
            Self::Restart {
                process_ref,
                runtime_ref,
                request_ref,
            } => vec![process_ref, runtime_ref, request_ref],
        };
        if refs.into_iter().all(digest) {
            Ok(())
        } else {
            Err(error("invalid-configuration"))
        }
    }
}
struct Live {
    endpoint: Arc<Mutex<Option<launch::Endpoint>>>,
    value: Arc<Mutex<ProcessSnapshot>>,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Live {
    fn stop(&mut self) -> Result<(), LocalInferenceError> {
        if self.worker.is_some() {
            let mut state = self
                .value
                .lock()
                .map_err(|_| error("process-state-unavailable"))?;
            if matches!(state.state, ProcessState::Loading | ProcessState::Ready) {
                state.state = ProcessState::Stopping;
            }
        }
        self.cancel.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| error("process-worker-failed"))?;
        }
        Ok(())
    }
}
impl Drop for Live {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[derive(Default)]
struct Owner {
    requests: inference::Requests,
    live: Option<Live>,
    accepted: BTreeMap<String, (String, String)>,
}
impl Owner {
    fn inspect(&self) -> Result<ProcessReply, LocalInferenceError> {
        Ok(ProcessReply {
            process: self.live.as_ref().map(|l| snapshot(&l.value)).transpose()?,
            rejection: None,
        })
    }
    fn stop(&mut self, reference: &str) -> Result<ProcessReply, LocalInferenceError> {
        let current = self
            .live
            .as_mut()
            .ok_or_else(|| error("process-not-found"))?;
        if snapshot(&current.value)?.process_ref != reference {
            return Err(error("process-generation-stale"));
        }
        current.stop()?;
        self.inspect()
    }
    fn start(
        &mut self,
        root: &Path,
        runtime: &str,
        request: &str,
        predecessor: Option<&str>,
    ) -> Result<ProcessReply, LocalInferenceError> {
        let binding = crate::configuration::identity(
            "runtime/process/request/binding/1",
            &(runtime, predecessor),
        )?;
        if let Some((accepted_runtime, accepted_process)) = self.accepted.get(request) {
            if accepted_runtime != &binding {
                return Err(error("process-request-conflict"));
            }
            let reply = self.inspect()?;
            if reply
                .process
                .as_ref()
                .is_some_and(|p| &p.process_ref == accepted_process)
            {
                return Ok(reply);
            }
            return Err(error("process-generation-retired"));
        }
        if let Some(live) = &self.live {
            let current = snapshot(&live.value)?;
            if matches!(
                current.state,
                ProcessState::Loading | ProcessState::Ready | ProcessState::Stopping
            ) {
                if current.runtime_ref != runtime {
                    return Err(error("process-capacity-exceeded"));
                }
                if self.accepted.len() >= 128 {
                    return Err(error("process-request-capacity"));
                }
                RuntimeRegistry::open_existing(root)?.record_process_request(
                    request,
                    &binding,
                    &current.process_ref,
                )?;
                self.accepted
                    .insert(request.into(), (binding, current.process_ref));
                return self.inspect();
            }
        }
        if self.accepted.len() >= 128 {
            return Err(error("process-request-capacity"));
        }
        // Validation is read-only; no model launch or implicit distribution derivation.
        let registry = RuntimeRegistry::open_existing(root)?;
        registry.runtime(runtime)?;
        if let Some(mut old) = self.live.take() {
            old.stop()?;
        }
        let reference = service::nonce()?;
        registry.record_process_request(request, &binding, &reference)?;
        let value = Arc::new(Mutex::new(ProcessSnapshot {
            runtime_ref: runtime.into(),
            process_ref: reference.clone(),
            state: ProcessState::Loading,
            reason: None,
            termination: None,
            rss_bytes: 0,
            peak_rss_bytes: 0,
            threads: 0,
            loaded_artifact_refs: vec![],
            observed_host_libraries: vec![],
        }));
        let cancel = Arc::new(AtomicBool::new(false));
        let endpoint = Arc::new(Mutex::new(None));
        let worker_endpoint = endpoint.clone();
        let (worker_value, worker_cancel, worker_root) =
            (value.clone(), cancel.clone(), root.to_path_buf());
        let worker = std::thread::Builder::new()
            .name("zixcel-runtime".into())
            .spawn(move || {
                let result = launch::run(
                    &worker_root,
                    &worker_value,
                    &worker_cancel,
                    &worker_endpoint,
                );
                if let Ok(mut state) = worker_value.lock() {
                    state.state = if result.is_ok() {
                        ProcessState::Stopped
                    } else {
                        ProcessState::Failed
                    };
                    state.reason = result.err().map(|e| e.code().into());
                    state.rss_bytes = 0;
                    state.threads = 0;
                }
            })
            .map_err(|_| error("process-worker-unavailable"))?;
        self.accepted.insert(request.into(), (binding, reference));
        self.live = Some(Live {
            endpoint,
            value,
            cancel,
            worker: Some(worker),
        });
        self.inspect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_replay_and_stale_stop_do_not_launch_or_change_generation() {
        let state = ProcessSnapshot {
            runtime_ref: "a".repeat(64),
            process_ref: "b".repeat(64),
            state: ProcessState::Ready,
            reason: None,
            termination: None,
            rss_bytes: 1,
            peak_rss_bytes: 1,
            threads: 1,
            loaded_artifact_refs: vec![],
            observed_host_libraries: vec![],
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let mut owner = Owner {
            requests: inference::Requests::default(),
            live: Some(Live {
                endpoint: Arc::new(Mutex::new(None)),
                value: Arc::new(Mutex::new(state.clone())),
                cancel: cancel.clone(),
                worker: None,
            }),
            accepted: BTreeMap::from([(
                "c".repeat(64),
                (
                    crate::configuration::identity(
                        "runtime/process/request/binding/1",
                        &(&state.runtime_ref, None::<&str>),
                    )
                    .expect("binding"),
                    state.process_ref.clone(),
                ),
            )]),
        };
        assert_eq!(
            owner
                .start(
                    Path::new("/nonexistent"),
                    &state.runtime_ref,
                    &"c".repeat(64),
                    None
                )
                .expect("replay")
                .process,
            Some(state.clone())
        );
        assert_eq!(
            owner
                .start(
                    Path::new("/nonexistent"),
                    &"d".repeat(64),
                    &"c".repeat(64),
                    None
                )
                .expect_err("conflict")
                .code(),
            "process-request-conflict"
        );
        assert_eq!(
            owner.stop(&"d".repeat(64)).expect_err("stale").code(),
            "process-generation-stale"
        );
        assert!(!cancel.load(Ordering::Acquire));
        assert_eq!(owner.inspect().expect("read").process, Some(state));
    }
}
