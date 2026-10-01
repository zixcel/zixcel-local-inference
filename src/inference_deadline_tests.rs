//! Hatter ecosystem 2026: owner execution budget, not cross-process UTC authority.
use super::*;

#[test]
fn prepared_request_has_no_creator_clock_execution_authority() {
    let request = specimen(&"b".repeat(64));
    let wire = serde_json::to_value(&request).expect("prepared wire");
    assert!(
        wire.get("deadline_ms").is_none(),
        "creator UTC is not an execution deadline"
    );
    assert_eq!(wire["options"]["execution_budget_ms"], 30_000);
    // Exact preparation repeats the same attempt, not a new timestamp-derived ID.
    let repeated = specimen(&"b".repeat(64));
    assert_eq!(request.request_ref(), repeated.request_ref());
    let mut legacy = wire;
    legacy["deadline_ms"] = serde_json::json!(1_789_198_174_170_u64);
    assert!(serde_json::from_value::<InferenceRequest>(legacy).is_err());
}

#[test]
fn invalid_execution_budgets_and_observer_timeouts_remain_distinct() {
    let request = specimen(&"b".repeat(64));
    for budget in [0, 30_001, u32::MAX] {
        let mut invalid = request.clone();
        invalid.options.execution_budget_ms = budget;
        assert_eq!(
            invalid.validate(),
            Err(InferenceError::InvalidExecutionBudget)
        );
    }
    let wire = serde_json::to_value(&request).expect("wire");
    for malformed in [
        serde_json::json!(-1),
        serde_json::json!(4_294_967_296_u64),
        serde_json::json!("30000"),
        serde_json::json!(null),
    ] {
        let mut invalid = wire.clone();
        invalid["options"]["execution_budget_ms"] = malformed;
        assert!(serde_json::from_value::<InferenceRequest>(invalid).is_err());
    }
    let mut missing = wire;
    missing["options"]
        .as_object_mut()
        .expect("options")
        .remove("execution_budget_ms");
    assert!(serde_json::from_value::<InferenceRequest>(missing).is_err());
    let control = VerificationControl::new(Duration::ZERO);
    assert_eq!(
        check_control(&control),
        Err(InferenceError::TransportTimeout)
    );
    control.cancel();
    assert_eq!(check_control(&control), Err(InferenceError::Cancelled));
}
