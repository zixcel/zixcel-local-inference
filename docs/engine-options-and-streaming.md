# Rust engine options and streaming boundaries

The library provides Rust-only selection and event-stream primitives.
It does not activate ONNX/OpenVINO execution, convert weights, or make a catalog
artifact executable. No Python process, module or package is used by these APIs.

## Ownership

| Responsibility | Owner |
| --- | --- |
| Monthly JPY planning from caller rates, compute/storage/network/service items and assumptions | `neo-erp/cost-estimator` |
| Token usage and per-token integer cost components, including cache/reasoning | `zixcel-cost` |
| Verified model/runtime identity, model selection, decoding, provisional text and terminal usage | `zixcel-local-inference` |
| Socket delivery, response byte limits, cancellation polling and transport backpressure | `crowsi-transport-foundation` |

The cost-estimator application and token estimation share calculation primitives, but their units,
categories, rounding and evidence contracts differ. A caller can supply token
cost as a compute planning item when it explicitly declares micro-JPY and a
monthly usage assumption. No currency conversion or provider price lookup is
implicit. Performance measurements are durations, not monetary estimates.
Do not move monetary calculation into the inference adapter or Crowsi.

## Selection options

`engine_options::EngineOptions::from_json` accepts a closed bounded document:

```json
{"engine":"onnxruntime","provider":"openvino"}
```

`selection()` returns the format and engine constraints for
`CatalogStore::acquisition_plan`. It does not carry out installation or launch.

| Engine / provider | Required catalog format / engine | Current physical execution |
| --- | --- | --- |
| `llama.cpp` / `cpu` | `gguf` / `llama.cpp` | Existing exact-revision, admitted CPU launcher; host/artifact checks still required |
| `onnxruntime` / `cpu`, `cuda`, `directml`, `openvino` | `onnx` / `onnxruntime` | Native adapter not implemented |
| `openvino` / `cpu`, `openvino` | `openvino-ir` / `openvino` | Native adapter not implemented; Intel device selection still to define |

OpenVINO as an ONNX execution provider keeps ONNX model format. Native OpenVINO
IR is a different choice. No automatic fallback or model conversion occurs.
`require_launch_adapter()` rejects unimplemented engines. It is an API guard,
not a live hardware probe. The existing physical launcher separately rejects
all engines other than its pinned llama.cpp configuration. The new option API
is not yet a CLI flag or a persisted runtime-admission schema.

Native ONNX/OpenVINO adoption needs a verified multi-file bundle for graph,
external weights, tokenizer and configuration. Current artifact placement is
single-file and executable admission accepts GGUF/ELF, not these bundles.
Engine labels alone must never set `inferenceReady` to true. Backend-specific
Rust adapters must bind those exact bundle identities and enforce cancellation,
deadline, model context, token counts and process lifecycle before activation.

## Streaming

Crowsi `process::guarded_tcp_stream` delivers byte chunks through a synchronous
receiver, with one connection, one absolute deadline and a total response cap.
Receiver rejection ends delivery. Its callback must return promptly; it cannot
be preempted by the transport. EOF proves transport completion only.

Zixcel `generation_stream::GenerationStream` consumes bounded NDJSON after an
engine adapter has decoded its backend protocol. It uses Crowsi's framing
decoder, validates contiguous delta sequences and a terminal usage event, and
rejects malformed, oversized, partial or post-terminal data. Deltas are
provisional. Only clean EOF plus valid terminal usage produces a receipt/cost.
Chunk counts are never treated as token counts. Monotonic first-text and total
durations are observations, not GPU performance predictions.

`GenerationReceipt` is an adapter-local observation without request/process
identity. It must not replace `InferenceResult` or authorize result adoption.
The adapter must bind the stream to its accepted request and live owner before
the existing physical result validation boundary.

This is an adapter-facing Rust contract, not an SSE HTTP parser. The existing
llama.cpp adapter still requests `stream:false` and IPC v2 still returns a
completed result. Live caller/UI streaming requires backend SSE/chunked decoding
and a reviewed IPC observation contract retaining request/process identities,
execution deadlines and cancellation; it has not been enabled by these changes.

## Native implementation feasibility

Intel maintains [Rust OpenVINO bindings](https://github.com/intel/openvino-rs),
including GenAI bindings and runtime linking. They use native OpenVINO libraries.
[ONNX Runtime APIs](https://onnxruntime.ai/docs/api/) classify Rust bindings as
community APIs; its [GenAI interface](https://onnxruntime.ai/docs/genai/) provides
native tokenization/generation APIs. Rust bindings to the reviewed native APIs
can avoid Python in the product, but are not pure-Rust engine implementations.

The [OpenVINO ONNX execution provider](https://onnxruntime.ai/docs/execution-providers/OpenVINO-ExecutionProvider.html)
is a provider choice requiring the corresponding native distribution and device
support. PyTorch/Transformers conversion scripts from Devino are not adopted:
a Python-free product should consume explicitly reviewed preconverted bundles
or add a narrowly supported Rust conversion path with equivalence tests.

The local tests use synthetic signed catalogs and Rust loopback sockets, not
real ONNX/OpenVINO model weights. Real model equivalence, native library licence/
redistribution, device support and locked distribution validation remain gates.
