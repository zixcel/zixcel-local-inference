# Physical inference execution (IPC v2)

The provider accepts bounded text and an opaque caller context digest. Callers
own application admission, repository fences, type interpretation and adoption.
The physical owner enforces exact runtime/process identity, immutable replay,
output bounds, cancellation and a finite monotonic execution budget.

Use `InferenceRequest::prepare(process, context_digest, input, options)` to bind
a request to the current ready process. `submit` accepts once; `wait` and
`inspect` observe only that accepted request. An ambiguous acceptance reply must
be recovered by inspection rather than automatic resubmission. Exact receipt
lookup and cancellation retain the original request/process identity.

The private same-OS-owner socket does not authenticate remote clients. Callers
must enforce their authorization policy before access. No inference result
implicitly authorizes effects or writes application meaning.

Text is limited to 4096 UTF-8 bytes, request/response bodies to 65536 bytes,
output to 1–256 tokens and execution budgets to 1–30000 ms. The owner starts
one monotonic budget at acceptance; observer/transport timeouts do not renew it.
At most 128 receipts are retained per live process, with one active computation
and no hidden queue. Owner death loses volatile results; lifecycle restart is
an explicit owner operation. IPC v1 bodies are rejected without compatibility
fallback or automatic state migration.
