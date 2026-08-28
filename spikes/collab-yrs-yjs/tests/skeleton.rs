use collab_yrs_yjs_spike::{InputError, InputLimits, IsolationKind, REQUIRED_RUST_ISOLATION_KIND, metadata};

#[test]
fn locks_candidate_metadata() {
    let metadata = metadata();
    assert_eq!(metadata.candidate, "yrs-yjs");
    assert_eq!(metadata.rust_engine, "yrs");
    assert_eq!(metadata.rust_engine_version, "0.27.3");
}

#[test]
fn guards_update_input_before_engine_apply() {
    let limits = InputLimits {
        snapshot_bytes_max: 8,
        update_bytes_max: 2,
    };
    let result = limits.validate_update(&[1, 2, 3]);
    assert!(matches!(
        result,
        Err(InputError::LimitExceeded {
            input: "update",
            actual_bytes: 3,
            max_bytes: 2
        })
    ));
}

#[test]
fn requires_a_terminable_rust_instance() {
    assert_eq!(REQUIRED_RUST_ISOLATION_KIND, IsolationKind::TerminableInstance);
    assert_ne!(REQUIRED_RUST_ISOLATION_KIND, IsolationKind::AsyncTimeout);
}
