//! Bounded physical text inference. Callers own semantic admission and result interpretation.
use crate::VerificationControl;
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};
use zixcel_inference_cost::{CostEstimate, CostRateCard, TokenUsage};
pub const MAX_REQUEST_BYTES: usize = 65_536;
pub const MAX_RESPONSE_BYTES: usize = 65_536;
pub const MAX_REQUESTS: usize = 128;
pub const MAX_EXECUTION_BUDGET_MS: u32 = 30_000;
pub const MAX_TEXT_BYTES: usize = 4096;
#[cfg(test)]
#[path = "inference_deadline_tests.rs"]
mod deadline_tests;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InferenceError {
    InvalidRequest,
    InvalidTypedInput,
    TargetMismatch,
    RuntimeNotFound,
    RuntimeNotReady,
    RuntimeUnavailable,
    StaleRuntimeProcess,
    RuntimeProcessMismatch,
    InvalidExecutionBudget,
    ExecutionTimedOut,
    TransportTimeout,
    Cancelled,
    CapacityExceeded,
    UnknownRequest,
    BackendRejected,
    BackendUnavailable,
    BackendProtocolError,
    BackendOutputInvalid,
    OutputLimitExceeded,
    CostAccountingError,
}
impl std::fmt::Display for InferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for InferenceError {}
type Result<T> = std::result::Result<T, InferenceError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceOptions {
    pub max_output_tokens: u16,
    /// Finite physical duration, bound once by the execution owner at acceptance.
    /// Neither a queue expiry nor an observer/transport timeout.
    pub execution_budget_ms: u32,
    /// Explicit estimate only; no default price or billing claim.
    pub cost_rates: Option<CostRateCard>,
}

/// A physical request bound to an opaque caller context digest and one process incarnation.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceRequest {
    pub(crate) request_ref: String,
    pub(crate) context_digest: String,
    pub(crate) runtime_ref: String,
    pub(crate) process_ref: String,
    pub(crate) input: String,
    pub(crate) options: InferenceOptions,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestReference {
    pub request_ref: String,
    pub process_ref: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReceiptStatus {
    Running,
    Complete { result_ref: String },
    Failed(InferenceError),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceResult {
    pub result_ref: String,
    pub request_ref: String,
    pub context_digest: String,
    pub runtime_ref: String,
    pub process_ref: String,
    pub output: String,
    pub usage: TokenUsage,
    pub usage_coverage: UsageCoverage,
    pub cost: Option<CostEstimate>,
    pub elapsed_ms: u64,
    pub accepted_at_epoch_ms: Option<u64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageCoverage {
    InputAndOutput,
}
fn identity<T: Serialize>(domain: &str, value: &T) -> Result<String> {
    crate::configuration::identity(domain, value).map_err(|_| InferenceError::InvalidRequest)
}
impl InferenceOptions {
    pub fn validate(&self) -> Result<()> {
        if self.execution_budget_ms == 0 || self.execution_budget_ms > MAX_EXECUTION_BUDGET_MS {
            return Err(InferenceError::InvalidExecutionBudget);
        }
        if self.max_output_tokens == 0 || self.max_output_tokens > 256 {
            return Err(InferenceError::InvalidRequest);
        }
        if let Some(rates) = &self.cost_rates {
            if [&rates.schema, &rates.unit, &rates.source, &rates.as_of]
                .iter()
                .any(|s| s.len() > 512)
                || rates.cached_input_micro_units_per_token != 0
                || rates.cache_write_input_micro_units_per_token != 0
                || rates.reasoning_output_micro_units_per_token != 0
            {
                return Err(InferenceError::CostAccountingError);
            }
            rates
                .validate()
                .map_err(|_| InferenceError::CostAccountingError)?;
        }
        Ok(())
    }
}

impl InferenceRequest {
    pub fn prepare(
        process: &crate::process::ProcessSnapshot,
        context_digest: String,
        input: String,
        options: InferenceOptions,
    ) -> Result<Self> {
        if process.state != crate::process::ProcessState::Ready {
            return Err(InferenceError::RuntimeNotReady);
        }
        let mut value = Self {
            request_ref: String::new(),
            context_digest,
            runtime_ref: process.runtime_ref.clone(),
            process_ref: process.process_ref.clone(),
            input,
            options,
        };
        value.request_ref = value.identity()?;
        value.validate()?;
        Ok(value)
    }
    pub fn request_ref(&self) -> &str {
        &self.request_ref
    }
    pub fn context_digest(&self) -> &str {
        &self.context_digest
    }
    pub fn reference(&self) -> RequestReference {
        RequestReference {
            request_ref: self.request_ref.clone(),
            process_ref: self.process_ref.clone(),
        }
    }
    fn identity(&self) -> Result<String> {
        identity(
            "runtime/inference/physical-request/2",
            &(
                &self.context_digest,
                &self.runtime_ref,
                &self.process_ref,
                &self.input,
                &self.options,
            ),
        )
    }
    pub fn validate(&self) -> Result<()> {
        self.options.validate()?;
        if !crate::process::digest(&self.runtime_ref)
            || !crate::process::digest(&self.process_ref)
            || !crate::process::digest(&self.context_digest)
            || self.input.len() > MAX_TEXT_BYTES
            || serde_json::to_vec(self)
                .map_err(|_| InferenceError::InvalidRequest)?
                .len()
                > MAX_REQUEST_BYTES
            || self.identity()? != self.request_ref
        {
            return Err(InferenceError::InvalidRequest);
        }
        Ok(())
    }
}
impl InferenceResult {
    pub(crate) fn seal(&mut self) -> Result<()> {
        self.result_ref.clear();
        self.result_ref = identity("runtime/inference/physical-result/2", self)?;
        Ok(())
    }
    pub fn validate(&self, request: &InferenceRequest) -> Result<()> {
        request.validate()?;
        if self.output.len() > MAX_TEXT_BYTES
            || serde_json::to_vec(self)
                .map_err(|_| InferenceError::BackendOutputInvalid)?
                .len()
                > MAX_RESPONSE_BYTES
        {
            return Err(InferenceError::OutputLimitExceeded);
        }
        if self.usage.output_tokens > u64::from(request.options.max_output_tokens)
            || self.usage.cached_input_tokens != 0
            || self.usage.cache_write_input_tokens != 0
            || self.usage.reasoning_output_tokens != 0
        {
            return Err(InferenceError::BackendOutputInvalid);
        }
        let expected = request
            .options
            .cost_rates
            .as_ref()
            .map(|rates| zixcel_inference_cost::estimate(&self.usage, rates))
            .transpose()
            .map_err(|_| InferenceError::CostAccountingError)?;
        let mut sealed = self.clone();
        sealed.seal()?;
        if self.cost != expected
            || sealed.result_ref != self.result_ref
            || self.context_digest != request.context_digest
            || self.request_ref != request.request_ref
            || self.runtime_ref != request.runtime_ref
            || self.process_ref != request.process_ref
        {
            return Err(InferenceError::BackendOutputInvalid);
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum Command {
    Infer {
        request: Box<InferenceRequest>,
    },
    InferenceInspect {
        request_ref: String,
    },
    InferenceCancel {
        request_ref: String,
    },
    Receipt {
        reference: RequestReference,
        cancel: bool,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", content = "value", deny_unknown_fields)]
pub(crate) enum Reply {
    Running,
    Complete(Box<InferenceResult>),
    Failed(InferenceError),
}

/// Exact replay is only available in this live owner. Owner death loses results;
/// its process incarnation is retired. This function never retries execution.
/// # Errors
/// Exact process, cancellation, deadline and backend errors are propagated.
pub fn execute(
    root: &Path,
    request: &InferenceRequest,
    control: &VerificationControl,
) -> Result<InferenceResult> {
    submit(root, request, control)?;
    wait(root, request, control)
}

fn check_control(control: &VerificationControl) -> Result<()> {
    control.check().map_err(|e| {
        if e.code() == "cancelled" {
            InferenceError::Cancelled
        } else {
            InferenceError::TransportTimeout
        }
    })
}

/// Submit one bounded physical request. Callers must enforce their admission policy.
pub fn submit(
    root: &Path,
    request: &InferenceRequest,
    control: &VerificationControl,
) -> Result<()> {
    check_control(control)?;
    request.validate()?;
    match crate::process::inference_request(
        root,
        &Command::Infer {
            request: Box::new(request.clone()),
        },
    )? {
        Reply::Failed(error) => Err(error),
        Reply::Running => Ok(()),
        Reply::Complete(result) => result.validate(request),
    }
}

/// Wait only for an exact already-submitted physical result. This never sends
/// Infer, interprets or adopts the output.
/// After waiting the semantic owner must revalidate its expected state before
/// interpreting/admitting. A lost volatile receipt remains explicitly lost.
/// # Errors
/// Cancellation, deadline, missing receipt and physical/result failures reject.
pub fn wait(
    root: &Path,
    request: &InferenceRequest,
    control: &VerificationControl,
) -> Result<InferenceResult> {
    request.validate()?;
    loop {
        if let Err(e) = check_control(control) {
            // Observation expiry does not cancel or renew physical execution.
            if e == InferenceError::Cancelled {
                let _ = crate::process::inference_request(
                    root,
                    &Command::InferenceCancel {
                        request_ref: request.request_ref.clone(),
                    },
                );
            }
            return Err(e);
        }
        if let Some(result) = inspect(root, request)? {
            return Ok(result);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Read-only exact retained physical observation. It is never an adoption and
/// never sends an execution command. Useful after loss of an acceptance reply.
/// # Errors
/// Unknown/lost requests and unavailable owners or invalid result identities reject.
pub fn inspect(root: &Path, request: &InferenceRequest) -> Result<Option<InferenceResult>> {
    request.validate()?;
    match crate::process::inference_request(
        root,
        &Command::InferenceInspect {
            request_ref: request.request_ref.clone(),
        },
    )? {
        Reply::Running => Ok(None),
        Reply::Failed(e) => Err(e),
        Reply::Complete(result) => {
            result.validate(request)?;
            Ok(Some(*result))
        }
    }
}

/// Exact request cancellation, not a runtime stop. A final result remains final.
/// # Errors
/// Unknown requests and unavailable owners reject; no other request is cancelled.
pub fn cancel(root: &Path, request: &InferenceRequest) -> Result<()> {
    request.validate()?;
    match crate::process::inference_request(
        root,
        &Command::InferenceCancel {
            request_ref: request.request_ref.clone(),
        },
    )? {
        Reply::Failed(InferenceError::UnknownRequest) => Err(InferenceError::UnknownRequest),
        _ => Ok(()),
    }
}

impl RequestReference {
    pub(crate) fn validate(&self) -> Result<()> {
        if !crate::process::digest(&self.request_ref) || !crate::process::digest(&self.process_ref)
        {
            return Err(InferenceError::InvalidRequest);
        }
        Ok(())
    }
}

/// Read an exact physical receipt by identity only. No Infer operation, input
/// reconstruction, result-body exposure or automatic process replacement.
/// # Errors
/// Malformed identities or unavailable physical transport remain owner errors.
pub fn inspect_reference(root: &Path, reference: &RequestReference) -> Result<ReceiptStatus> {
    receipt(root, reference, false)
}

/// Request cancellation of exactly this retained request/process incarnation.
/// A Running response means cancellation is pending, not capacity release.
/// # Errors
/// Malformed identities or unavailable physical transport remain owner errors.
pub fn cancel_reference(root: &Path, reference: &RequestReference) -> Result<ReceiptStatus> {
    receipt(root, reference, true)
}
fn receipt(root: &Path, reference: &RequestReference, cancel: bool) -> Result<ReceiptStatus> {
    reference.validate()?;
    match crate::process::inference_request(
        root,
        &Command::Receipt {
            reference: reference.clone(),
            cancel,
        },
    )? {
        Reply::Running => Ok(ReceiptStatus::Running),
        Reply::Complete(result) => {
            if result.request_ref != reference.request_ref
                || result.process_ref != reference.process_ref
            {
                return Err(InferenceError::InvalidRequest);
            }
            Ok(ReceiptStatus::Complete {
                result_ref: result.result_ref,
            })
        }
        Reply::Failed(error) => Ok(ReceiptStatus::Failed(error)),
    }
}

#[cfg(test)]
pub(crate) fn specimen(process: &str) -> InferenceRequest {
    specimen_named(process, "default")
}
#[cfg(test)]
pub(crate) fn specimen_named(process: &str, id: &str) -> InferenceRequest {
    specimen_budget(process, id, 30_000)
}
#[cfg(test)]
pub(crate) fn specimen_budget(process: &str, id: &str, budget_ms: u32) -> InferenceRequest {
    let snapshot = crate::process::ProcessSnapshot {
        runtime_ref: "a".repeat(64),
        process_ref: process.into(),
        state: crate::process::ProcessState::Ready,
        reason: None,
        termination: None,
        rss_bytes: 0,
        peak_rss_bytes: 0,
        threads: 0,
        loaded_artifact_refs: vec![],
        observed_host_libraries: vec![],
    };
    let context = identity("test/context", &id).expect("context");
    InferenceRequest::prepare(
        &snapshot,
        context,
        "test prompt".into(),
        InferenceOptions {
            max_output_tokens: 8,
            execution_budget_ms: budget_ms,
            cost_rates: None,
        },
    )
    .expect("bounded request")
}
