mod support;
use support::{catalog, model, write_signed};
use zixcel_local_inference::{
    CatalogStore,
    engine_options::{EngineOptions, ExecutionProvider, ModelEngine},
};

#[test]
fn model_selection_distinguishes_format_engine_and_provider_without_fallback() {
    let temp = tempfile::tempdir().expect("temp");
    let mut descriptor = model("choice", "approved");
    let original = descriptor.releases[0].artifacts[0].clone();
    for (id, format, engine) in [
        ("onnx", "onnx", "onnxruntime"),
        ("ir", "openvino-ir", "openvino"),
    ] {
        let mut artifact = original.clone();
        artifact.id = id.into();
        artifact.format = format.into();
        artifact.runtime_engines = vec![engine.into()];
        descriptor.releases[0].artifacts.push(artifact);
    }
    let envelope = temp.path().join("catalog.json");
    let (key, public) = write_signed(&envelope, &catalog(1, vec![descriptor]), 4);
    let store = CatalogStore::provision(&temp.path().join("state")).expect("store");
    store
        .add_file_source("fixture", 100, &envelope, &key, &public)
        .expect("source");
    store.refresh("fixture", None).expect("refresh");
    for (engine, provider, artifact) in [
        (ModelEngine::LlamaCpp, ExecutionProvider::Cpu, "artifact-1"),
        (ModelEngine::OnnxRuntime, ExecutionProvider::Cpu, "onnx"),
        (
            ModelEngine::OnnxRuntime,
            ExecutionProvider::OpenVino,
            "onnx",
        ),
        (ModelEngine::OpenVino, ExecutionProvider::Cpu, "ir"),
    ] {
        let options = EngineOptions { engine, provider };
        let plan = store
            .acquisition_plan(
                "choice",
                &temp.path().join("models"),
                &options.selection().expect("selection"),
            )
            .expect("plan");
        assert_eq!(plan.artifact_id, artifact);
        assert!(!plan.automatic_execution);
        assert_eq!(
            options.require_launch_adapter().is_ok(),
            engine == ModelEngine::LlamaCpp
        );
    }
}

#[test]
fn invalid_options_and_unknown_fields_fail_closed() {
    for bytes in [
        br#"{"engine":"openvino","provider":"cuda"}"#.as_slice(),
        br#"{"engine":"llama.cpp","provider":"cpu","fallback":true}"#,
        br#"{"engine":"onnxruntime","engine":"llama.cpp","provider":"cpu"}"#,
        br#"{"engine":"automatic","provider":"cpu"}"#,
    ] {
        assert!(EngineOptions::from_json(bytes).is_err());
    }
    assert!(EngineOptions::from_json(&vec![b' '; 4097]).is_err());
    let options = EngineOptions::from_json(br#"{"engine":"onnxruntime","provider":"openvino"}"#)
        .expect("options");
    assert_eq!(
        options.selection().expect("selection").format.as_deref(),
        Some("onnx")
    );
    assert_eq!(
        options
            .require_launch_adapter()
            .expect_err("not implemented")
            .code(),
        "native-adapter-not-implemented"
    );
}
