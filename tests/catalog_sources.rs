mod support;

use std::fs;

use tempfile::tempdir;
use zixcel_local_inference::CatalogStore;

use support::{catalog, model, write_signed};

#[test]
fn refreshed_signed_catalog_changes_candidates_without_recompilation() {
    let temporary = tempdir().expect("temporary directory");
    let state = temporary.path().join("state");
    let envelope = temporary.path().join("catalog.signed.json");
    let (key_id, public_key) =
        write_signed(&envelope, &catalog(1, vec![model("alpha", "approved")]), 1);
    let store = CatalogStore::provision(&state).expect("catalog store");
    store
        .add_file_source("primary", 100, &envelope, &key_id, &public_key)
        .expect("source");
    store.refresh("primary", None).expect("refresh one");
    assert_eq!(store.candidates().expect("candidates").len(), 1);

    write_signed(
        &envelope,
        &catalog(
            2,
            vec![model("alpha", "approved"), model("beta", "review-required")],
        ),
        1,
    );
    store.refresh("primary", None).expect("refresh two");
    let candidates = store.candidates().expect("updated candidates");
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].id, "alpha");
    assert_eq!(candidates[1].id, "beta");
    assert_eq!(candidates[1].release_state, "review-required");
}

#[test]
fn signature_rollback_and_sequence_equivocation_fail_closed() {
    let temporary = tempdir().expect("temporary directory");
    let state = temporary.path().join("state");
    let envelope = temporary.path().join("catalog.signed.json");
    let (key_id, public_key) =
        write_signed(&envelope, &catalog(2, vec![model("alpha", "approved")]), 2);
    let store = CatalogStore::provision(&state).expect("catalog store");
    store
        .add_file_source("primary", 100, &envelope, &key_id, &public_key)
        .expect("source");
    store.refresh("primary", None).expect("refresh");

    write_signed(&envelope, &catalog(1, vec![model("alpha", "approved")]), 2);
    assert_eq!(
        store.refresh("primary", None).expect_err("rollback").code(),
        "catalog-rollback-rejected"
    );

    write_signed(
        &envelope,
        &catalog(2, vec![model("changed", "approved")]),
        2,
    );
    assert_eq!(
        store
            .refresh("primary", None)
            .expect_err("equivocation")
            .code(),
        "catalog-sequence-equivocation"
    );

    let bytes = fs::read(&envelope).expect("envelope");
    let mut envelope_value: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    envelope_value["signatureBase64"] = "AAAAAAAA".into();
    fs::write(
        &envelope,
        serde_json::to_vec_pretty(&envelope_value).expect("json bytes"),
    )
    .expect("tamper");
    assert_eq!(
        store
            .refresh("primary", None)
            .expect_err("signature")
            .code(),
        "catalog-signature-invalid"
    );
}

#[test]
fn one_source_cannot_change_catalog_identity_or_publish_empty_semantics() {
    let temporary = tempdir().expect("temporary directory");
    let state = temporary.path().join("state");
    let envelope = temporary.path().join("catalog.signed.json");
    let (key_id, public_key) =
        write_signed(&envelope, &catalog(1, vec![model("alpha", "approved")]), 9);
    let store = CatalogStore::provision(&state).expect("catalog store");
    store
        .add_file_source("primary", 100, &envelope, &key_id, &public_key)
        .expect("source");
    store.refresh("primary", None).expect("refresh one");

    let mut changed_identity = catalog(2, vec![model("alpha", "approved")]);
    changed_identity.catalog_id = "different-catalog".to_owned();
    write_signed(&envelope, &changed_identity, 9);
    assert_eq!(
        store
            .refresh("primary", None)
            .expect_err("identity change")
            .code(),
        "catalog-identity-changed"
    );

    let mut invalid = model("alpha", "approved");
    invalid.capabilities = vec![String::new()];
    write_signed(&envelope, &catalog(2, vec![invalid]), 9);
    assert_eq!(
        store
            .refresh("primary", None)
            .expect_err("empty semantics")
            .code(),
        "catalog-model-invalid"
    );
}

#[test]
fn remote_sources_emit_crowsi_delivery_and_accept_only_verified_delivery() {
    let temporary = tempdir().expect("temporary directory");
    let state = temporary.path().join("state");
    let delivered = temporary.path().join("delivered.signed.json");
    let (key_id, public_key) = write_signed(
        &delivered,
        &catalog(1, vec![model("remote", "approved")]),
        3,
    );
    let store = CatalogStore::provision(&state).expect("catalog store");
    store
        .add_remote_source(
            "remote-source",
            100,
            "https://catalog.example.test/models.json",
            &key_id,
            &public_key,
        )
        .expect("remote source");
    let request = store.delivery_request("remote-source").expect("request");
    assert_eq!(request.transport_owner, "crowsi");
    assert!(!request.url.is_empty());
    assert_eq!(
        store
            .refresh("remote-source", None)
            .expect_err("delivery required")
            .code(),
        "catalog-delivery-required"
    );
    store
        .refresh("remote-source", Some(&delivered))
        .expect("verified delivery");
    assert_eq!(store.candidates().expect("candidates")[0].id, "remote");
}

#[test]
fn same_priority_conflicts_are_not_resolved_by_directory_order() {
    let temporary = tempdir().expect("temporary directory");
    let state = temporary.path().join("state");
    let first = temporary.path().join("first.json");
    let second = temporary.path().join("second.json");
    let (first_key_id, first_key) =
        write_signed(&first, &catalog(1, vec![model("same", "approved")]), 4);
    let mut changed = model("same", "approved");
    changed.display_name = "Conflicting definition".to_owned();
    let (second_key_id, second_key) = write_signed(&second, &catalog(1, vec![changed]), 5);
    let store = CatalogStore::provision(&state).expect("catalog store");
    store
        .add_file_source("first", 100, &first, &first_key_id, &first_key)
        .expect("first source");
    store
        .add_file_source("second", 100, &second, &second_key_id, &second_key)
        .expect("second source");
    store.refresh("first", None).expect("first refresh");
    store.refresh("second", None).expect("second refresh");
    assert_eq!(
        store.models().expect_err("conflict").code(),
        "catalog-model-conflict"
    );
}
