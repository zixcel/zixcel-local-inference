# Exact CPU execution and foreground process ownership

Version: 0.10.0. Linux x86-64, fixed audited llama.cpp b10741 adapter.

CPU preparation is an explicit `prepareCpuDistribution` command using rule
`llama-9d817213a-x64-baseline-1`. It selects the audited baseline module by exact
SHA-256 and fixed build semantics, not filename or SIGILL. `admitCpuDistribution`
recomputes preparation, uses normal distribution admission and commits separate
derivation provenance to the same Graph database. Host SSE2 observation is not
part of distribution identity or semantic memory. The original distribution is
never modified. A new runtime is admitted explicitly with the same model reference.

Start the external owner explicitly:

```text
zixcel-local-inference runtime-serve --state ABSOLUTE_ADMISSION_REGISTRY
```

An interactive client that explicitly requests local inference may instead own
the bounded foreground supervisor:

```text
zixcel-local-inference runtime-supervise --state ABSOLUTE_ADMISSION_REGISTRY [--runtime EXACT_REF]
```

An explicit reference starts only that admitted runtime. Without one, the
registry must contain exactly one runtime. The same exclusive
owner/lock/resource boundary is retained. Zero runtimes reject with
`runtime-not-found`; an implicit choice among multiple runtimes rejects with
`runtime-selection-required`. It performs no model selection fallback and
terminates with its owning client.

This foreground process does not load a model until an exact Start. Hatter does
not spawn it. CLI and Hatter RPC use the same closed owner command:

```json
{"operation":"process","command":{"operation":"inspect"}}
```

The inner command schema is `schemas/runtime-process-command-v1.schema.json`.
Start requires an exact `runtimeRef` and caller-generated 256-bit `requestRef`.
Stop requires the exact `processRef`. Restart also requires a fresh requestRef;
the predecessor is bound into request identity. Transport success is not operation
success: inspect `observation.rejection` before treating a reply as accepted.
Rejections preserve the exact owner's reason, not a compatibility translation.

The service holds one OS lock and one runtime slot. A fresh random-domain identity
names each incarnation, never PID/address. Request deduplication is committed by
Graph before launch. Owner restart retains deduplication but restores no child or
Ready state: replay of a retired request rejects. 128 recorded requests is a hard
bounded capacity, not permission to discard deduplication records. Reads do not
initialize/repair endpoints or create models. Explicit owner startup reclaims
only its recorded incarnation layouts and stale socket after exclusive ownership.

Zixcel owns exact materialization, adapter arguments, CPU validation, private
loopback authentication, model health and maps observations. Crowsi owns bounded
framing/TCP exchange and parent-death/process-group termination. The child arms
PDEATHSIG before checking its creator, then revalidates exact native layout and
model bytes before exec. The creating thread remains alive until reap. Native
objects are hardlinked without changing their permissions; the GGUF object is
reused read-only, never copied. Loader environment is cleared; no PATH/LD/GGML
override or user-selected endpoint is accepted. The port must belong to the
owned child before any HTTP. Ready requires authenticated props matching the
incarnation, health, and all exact native artifacts observed in process maps.

One runtime process is enforced; CPU thread count is propagated, not OS affinity.
OS service threads are reported separately from configured compute threads.

`ProcessSnapshot.termination` is a volatile observation after the original Crowsi
process-group cleanup/reap: exit code or signal, and optionally a reported error
code from an exact bounded Zixcel CLI error envelope. The existing monitor drains
child stderr nonblockingly (at most 64 KiB per pass) and retains at most 4 KiB
temporarily; overflow, raw native text, additional fields and unsafe values are
discarded, never projected or persisted. No extra thread or runtime is created.
These diagnostics do not change request outcomes, readiness, retry decisions,
admission, or the existing verification/execution budgets. Missing diagnostics
are unknown, not success. The leader is never reaped before group cleanup.
RSS is sampled every 100 ms and a budget excess terminates the child. This is
cooperative enforcement, not a kernel hard RSS cap. Startup is bounded to 90 s
after child spawn; verification has a cancellable 30 s bound. Stop sends TERM,
then KILL to the owned process group before reaping (avoids leader PID reuse).
Output retention is zero; stdin/stdout/stderr cannot accumulate model logs.
IPC frames are <=16 KiB with total deadlines and no unbounded task queue.

This is technical NON_PP management only. It does not bind semantic inference,
create ModelRoutes/providers/Grants or return inference results. M4-C0 remains
separate. Package acceptance is not proven by this document; the product report
must include actual native and built-browser evidence for the exact publication.
