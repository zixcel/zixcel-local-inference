//! Explicit artifact selection. Selection never attests to execution readiness.
use crate::{LocalInferenceError, Selection};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelEngine {
    #[serde(rename = "llama.cpp")]
    LlamaCpp,
    #[serde(rename = "onnxruntime")]
    OnnxRuntime,
    #[serde(rename = "openvino")]
    OpenVino,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionProvider {
    Cpu,
    Cuda,
    #[serde(rename = "directml")]
    DirectMl,
    #[serde(rename = "openvino")]
    OpenVino,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngineOptions {
    pub engine: ModelEngine,
    pub provider: ExecutionProvider,
}

impl EngineOptions {
    /// Closed, bounded caller configuration; no backend imports or downloads.
    /// # Errors
    /// Unknown/duplicate fields, invalid combinations and oversized input reject.
    pub fn from_json(bytes: &[u8]) -> Result<Self, LocalInferenceError> {
        if bytes.len() > 4096 {
            return Err(LocalInferenceError::new("invalid-engine-options"));
        }
        let options: Self = serde_json::from_slice(bytes)
            .map_err(|_| LocalInferenceError::new("invalid-engine-options"))?;
        options.selection()?;
        Ok(options)
    }

    /// Maps the chosen engine to the exact catalog format/engine constraint.
    /// `OpenVINO` as an ONNX provider keeps the ONNX format and engine identity.
    /// # Errors
    /// Incompatible engine/provider combinations reject without fallback.
    pub fn selection(&self) -> Result<Selection, LocalInferenceError> {
        let (format, runtime) = match (self.engine, self.provider) {
            (ModelEngine::LlamaCpp, ExecutionProvider::Cpu) => ("gguf", "llama.cpp"),
            (ModelEngine::OnnxRuntime, _) => ("onnx", "onnxruntime"),
            (ModelEngine::OpenVino, ExecutionProvider::Cpu | ExecutionProvider::OpenVino) => {
                ("openvino-ir", "openvino")
            }
            _ => return Err(LocalInferenceError::new("invalid-engine-options")),
        };
        Ok(Selection {
            format: Some(format.into()),
            runtime_engine: Some(runtime.into()),
        })
    }

    /// Checks only the currently implemented launch adapter, not host readiness.
    /// Exact artifacts, runtime revision and CPU admission still require checks.
    /// # Errors
    /// ONNX/OpenVINO execution is not implemented and must not launch as llama.
    pub fn require_launch_adapter(&self) -> Result<(), LocalInferenceError> {
        self.selection()?;
        if self.engine != ModelEngine::LlamaCpp {
            return Err(LocalInferenceError::new("native-adapter-not-implemented"));
        }
        Ok(())
    }
}
