use std::fs;
use std::path::PathBuf;

#[test]
fn admission_schema_operations_match_closed_owner_parsing_and_serialization() {
    use zixcel_local_inference::RegistryCommand;
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../schemas/runtime-admission-command-v1.schema.json"
    ))
    .expect("owner schema");
    let branches = schema["oneOf"].as_array().expect("command branches");
    assert_eq!(branches.len(), 14);
    for branch in branches {
        assert_eq!(branch["additionalProperties"], false);
        let name = branch["properties"]["operation"]["const"]
            .as_str()
            .expect("operation");
        if name.starts_with("admit") {
            continue;
        }
        let mut value = serde_json::json!({"operation": name});
        if name=="process" {value["command"]=serde_json::json!({"operation":"inspect"});}
        if branch["properties"].get("reference").is_some() {
            value["reference"] = serde_json::json!("a".repeat(64));
        }
        if let Some(rule) = branch["properties"].get("rule") {
            value["rule"] = rule["const"].clone();
        }
        let command = RegistryCommand::from_json(&serde_json::to_vec(&value).expect("request"))
            .expect("closed owner parse");
        assert_eq!(
            serde_json::to_value(command).expect("owner serialization"),
            value
        );
        value["selectBestModel"] = serde_json::json!(true);
        assert!(
            RegistryCommand::from_json(&serde_json::to_vec(&value).expect("negative request"))
                .is_err()
        );
    }
}

#[test]
fn published_schema_documents_are_closed_and_use_the_runtime_contract_ids() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas");
    let expected = [
        (
            "signed-catalog-v1.schema.json",
            "zixcel://local-inference/signed-catalog/v1",
        ),
        (
            "catalog-v2.schema.json",
            "zixcel://local-inference/catalog/v2",
        ),
        (
            "acquisition-plan-v2.schema.json",
            "zixcel://local-inference/acquisition-plan/v2",
        ),
        (
            "runtime-route-v1.schema.json",
            "zixcel://local-inference/runtime-route/v1",
        ),
        (
            "catalog-source-v1.schema.json",
            "zixcel://local-inference/catalog-source/v1",
        ),
        (
            "catalog-delivery-request-v1.schema.json",
            "zixcel://local-inference/catalog-delivery-request/v1",
        ),
        (
            "installation-v2.schema.json",
            "zixcel://local-inference/installation/v2",
        ),
        (
            "candidate-list-v2.schema.json",
            "zixcel://local-inference/candidate-list/v2",
        ),
    ];
    for (filename, contract_id) in expected {
        let bytes = fs::read(root.join(filename)).expect("schema document");
        let schema: serde_json::Value = serde_json::from_slice(&bytes).expect("schema JSON");
        assert_eq!(schema["$id"], contract_id);
        assert_eq!(schema["additionalProperties"], false);
    }
}
