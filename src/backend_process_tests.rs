use super::*;
#[test]
fn resource_plan_never_raises_existing_limits_without_exercising_exhaustion() {
    assert_eq!(
        lowered(
            Rlimit {
                current: Some(16),
                maximum: Some(32)
            },
            64
        ),
        Rlimit {
            current: Some(16),
            maximum: Some(32)
        }
    );
    assert_eq!(
        lowered(
            Rlimit {
                current: None,
                maximum: None
            },
            8
        ),
        Rlimit {
            current: Some(8),
            maximum: Some(8)
        }
    );
    assert_eq!(
        lowered(
            Rlimit {
                current: Some(16),
                maximum: Some(32)
            },
            4
        ),
        Rlimit {
            current: Some(4),
            maximum: Some(4)
        }
    );
}
#[test]
fn invalid_resource_requests_reject_without_allocating_or_setting_limits() {
    let original = getrlimit(Resource::As);
    for bytes in [0, 1024, u64::MAX] {
        assert!(
            WorkerLimits {
                address_space_bytes: bytes,
                cpu_seconds: 5
            }
            .validate()
            .is_err()
        );
    }
    assert!(
        WorkerLimits {
            address_space_bytes: 256 * 1024 * 1024,
            cpu_seconds: 0
        }
        .validate()
        .is_err()
    );
    assert_eq!(getrlimit(Resource::As), original);
}
