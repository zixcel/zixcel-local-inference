//! llama-specific conversion lives entirely below the private launch adapter.
//! Hatter ecosystem 2026: enforce the original physical owner's monotonic budget.
use super::Endpoint;
use crate::inference::{
    InferenceError as E, InferenceRequest, InferenceResult, MAX_RESPONSE_BYTES,
};
use crate::process::{ProcessSnapshot, inference::ExecutionClock};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

pub(crate) fn infer(
    endpoint: &Endpoint,
    state: &Arc<Mutex<ProcessSnapshot>>,
    request: &InferenceRequest,
    cancel: &AtomicBool,
    clock: ExecutionClock,
) -> Result<InferenceResult, E> {
    request.validate()?;
    let check = || {
        if cancel.load(Ordering::Acquire) {
            return Err(E::Cancelled);
        }
        crate::process::inference::current(state, request)?;
        clock.check()
    };
    check()?;
    let prompt = &request.input;
    let body = serde_json::to_vec(&serde_json::json!({
        "prompt":prompt,"n_predict":request.options.max_output_tokens,
        "temperature":0,"stream":false,"cache_prompt":false,
    }))
    .map_err(|_| E::InvalidTypedInput)?;
    let headers = format!(
        "POST /completion HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        endpoint.key,
        body.len()
    );
    let mut bytes = headers.into_bytes();
    bytes.extend_from_slice(&body);
    let response = crowsi_transport_foundation::process::guarded_tcp_exchange(
        ([127, 0, 0, 1], endpoint.port).into(),
        &bytes,
        MAX_RESPONSE_BYTES,
        clock.deadline,
        || check().map_err(|_| std::io::ErrorKind::Interrupted.into()),
    );
    check()?;
    let response = response.map_err(|e| match e.kind() {
        std::io::ErrorKind::TimedOut => E::TransportTimeout,
        std::io::ErrorKind::InvalidData => E::OutputLimitExceeded,
        _ => E::BackendUnavailable,
    })?;
    let mut result = convert(request, &response)?;
    result.elapsed_ms =
        u64::try_from(clock.started.elapsed().as_millis()).map_err(|_| E::CostAccountingError)?;
    result.accepted_at_epoch_ms = clock.accepted_at_epoch_ms;
    result.seal()?;
    check()?;
    Ok(result)
}
fn body(bytes: &[u8]) -> Result<&[u8], E> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(E::OutputLimitExceeded);
    }
    let offset = bytes
        .windows(4)
        .position(|b| b == b"\r\n\r\n")
        .ok_or(E::BackendProtocolError)?;
    if offset > 8192 {
        return Err(E::BackendProtocolError);
    }
    let header = std::str::from_utf8(&bytes[..offset]).map_err(|_| E::BackendProtocolError)?;
    let mut lines = header.split("\r\n");
    let status = lines.next().ok_or(E::BackendProtocolError)?;
    if status.starts_with("HTTP/1.1 503 ") {
        return Err(E::BackendUnavailable);
    }
    if !status.starts_with("HTTP/1.1 200 ") {
        return Err(E::BackendRejected);
    }
    let mut length = None;
    for line in lines {
        let (key, value) = line.split_once(':').ok_or(E::BackendProtocolError)?;
        if key.eq_ignore_ascii_case("transfer-encoding") {
            return Err(E::BackendProtocolError);
        }
        if key.eq_ignore_ascii_case("content-length") {
            if length.is_some() {
                return Err(E::BackendProtocolError);
            }
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| E::BackendProtocolError)?,
            );
        }
    }
    let body = &bytes[offset + 4..];
    if length != Some(body.len()) {
        return Err(E::BackendProtocolError);
    }
    Ok(body)
}
pub(super) fn convert(request: &InferenceRequest, bytes: &[u8]) -> Result<InferenceResult, E> {
    let data: serde_json::Value =
        serde_json::from_slice(body(bytes)?).map_err(|_| E::BackendProtocolError)?;
    let text = data
        .get("content")
        .and_then(serde_json::Value::as_str)
        .ok_or(E::BackendOutputInvalid)?;
    let input = data
        .get("tokens_evaluated")
        .and_then(serde_json::Value::as_u64)
        .ok_or(E::CostAccountingError)?;
    let output = data
        .get("tokens_predicted")
        .and_then(serde_json::Value::as_u64)
        .ok_or(E::CostAccountingError)?;
    if output > u64::from(request.options.max_output_tokens) {
        return Err(E::OutputLimitExceeded);
    }
    if text.len() > crate::inference::MAX_TEXT_BYTES {
        return Err(E::OutputLimitExceeded);
    }
    let usage = zixcel_inference_cost::TokenUsage::new(input, 0, 0, output, 0)
        .map_err(|_| E::CostAccountingError)?;
    let cost = request
        .options
        .cost_rates
        .as_ref()
        .map(|rates| zixcel_inference_cost::estimate(&usage, rates))
        .transpose()
        .map_err(|_| E::CostAccountingError)?;
    Ok(InferenceResult {
        result_ref: String::new(),
        request_ref: request.request_ref.clone(),
        context_digest: request.context_digest.clone(),
        runtime_ref: request.runtime_ref.clone(),
        process_ref: request.process_ref.clone(),
        output: text.into(),
        usage,
        usage_coverage: crate::inference::UsageCoverage::InputAndOutput,
        cost,
        elapsed_ms: 0,
        accepted_at_epoch_ms: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn response(value: &serde_json::Value) -> Vec<u8> {
        let body = serde_json::to_vec(&value).expect("json");
        let mut bytes =
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
        bytes.extend(body);
        bytes
    }
    #[test]
    fn adapter_rejects_protocol_bounds_and_untyped_values_without_semantic_coercion() {
        let request = crate::inference::specimen(&"b".repeat(64));
        let value =
            serde_json::json!({"content":"short","tokens_evaluated":4,"tokens_predicted":2});
        let result = convert(&request, &response(&value)).expect("bounded response");
        assert_eq!(result.usage.input_tokens, 4);
        assert_eq!(result.usage.output_tokens, 2);
        assert_eq!(result.output, "short");
        assert_eq!(result.request_ref, request.request_ref);
        for bad in [
            serde_json::json!(null),
            serde_json::json!(42),
            serde_json::json!(["text"]),
        ] {
            let mut malformed = value.clone();
            malformed["content"] = bad;
            assert_eq!(
                convert(&request, &response(&malformed)),
                Err(E::BackendOutputInvalid)
            );
        }
        let mut oversized = value.clone();
        oversized["content"] = serde_json::json!("x".repeat(4097));
        assert_eq!(
            convert(&request, &response(&oversized)),
            Err(E::OutputLimitExceeded)
        );
        let mut count = value.clone();
        count["tokens_predicted"] = serde_json::json!(9);
        assert_eq!(
            convert(&request, &response(&count)),
            Err(E::OutputLimitExceeded)
        );
        let mut count = value;
        count["tokens_evaluated"] = serde_json::json!(-1);
        assert_eq!(
            convert(&request, &response(&count)),
            Err(E::CostAccountingError)
        );
        assert_eq!(
            convert(&request, b"HTTP/1.1 503 Unavailable\r\n\r\n"),
            Err(E::BackendUnavailable)
        );
        assert_eq!(
            convert(&request, b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nx"),
            Err(E::BackendProtocolError)
        );
        assert_eq!(
            convert(&request, &vec![0; MAX_RESPONSE_BYTES + 1]),
            Err(E::OutputLimitExceeded)
        );
    }
}
