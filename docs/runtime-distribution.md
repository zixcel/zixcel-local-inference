# Exact runtime distribution profile — 0.10.0

This supplements static ELF admission; it does not replace that valid profile.
No process, loader layout, network request or default model is created by reads.

## Identity and admission

`DynamicElf64` admits immutable ET_DYN components through the existing
same-descriptor SHA-256, size, format and double-hash materialization path.
`RuntimeDistribution` declares a flat relative layout of admitted artifact refs,
exact symbolic-link edges, entry executable, explicitly declared dynamic modules,
and required host ABI. Files, aliases, modules and host lists are sorted, unique,
bounded and closed. Alias cycles/escapes, omitted dependencies, unreferenced
components, unresolved SONAMEs and unspecified host dependencies reject.

`RuntimeDistributionRef` is the domain-separated digest of that definition.
Its transitive artifact refs include byte digests, sizes, format and provenance.
The existing RuntimeRef uses this exact ref as its implementation ref; static
implementations still reference their exact static artifact. Registry definitions
are the authority, not filenames or an executable-only digest. Distribution
admission uses the existing Graph CAS and original-receipt replay. No second store.

An explicitly curated module list includes dlopen components that DT_NEEDED
alone cannot discover. An ELF audit cannot prove that arbitrary native code will
never request another path. Native code is trusted through the reviewed artifact
provenance; process-level loader restrictions must be enforced before M4-B start.
Provenance is an owner assertion, not a fabricated publisher signature.

## Loader and host boundary

The profile accepts ELF64 little-endian x86_64/aarch64, ET_DYN, bounded program
headers, mapped dynamic strings and version requirements. It rejects legacy RPATH,
loader audit/filter objects, duplicate tags and any RUNPATH except `$ORIGIN`.
Owned dependencies require `$ORIGIN` on their requesting object. Linux glibc
interpreters are explicitly selected by architecture. System SONAMEs are a closed
allowlist (glibc/C++/math/GCC/OpenMP/OpenSSL ABI families), never arbitrary paths.
The exact set of required host symbol versions is verified against ELF VERNEED.
Host-installed bytes are not copied or treated as immutable implementation bytes.
Changing the host does not change distribution identity.

Before M4-B execution: materialize only this exact read-only layout; reverify
managed objects; reject unlisted files; clear LD_LIBRARY_PATH, LD_PRELOAD,
LD_AUDIT and backend search overrides; check host loader/SONAME/symbol/CPU
availability and bind its observation to the process incarnation. No uncontrolled
working-directory/ambient PATH selection. Exact admission is not a host readiness
claim: `verifyRuntime` returns `runtime-host-verification-required` for dynamic
profiles until the process owner's availability boundary exists. `verifyDistribution`
verifies owned bytes and closure read-only. No implicit static-only workaround.

## Format-specific GGUF budget

Generic/static ELF prefix budget remains 2 MiB. GGUF is instead read through a
64 KiB buffer with a 64 MiB cumulative metadata/descriptor ceiling, 8 GiB artifact
ceiling, 4,096 KV entries, 16,384 tensors, 262,144 array elements, 65,536-byte
individual strings, 256-byte keys, 64-byte tensor names and four dimensions.
No nested arrays. Existing alignment/extent/overlap/type/digest checks remain.
Key/name indexes are count- and length-bounded; they do not store token arrays.
Cancellation/deadline is checked for reads and every field. The 64 MiB budget
separates tokenizer/descriptor volume from working memory; it is a deliberately
finite supported profile, not a claim to support every GGUF model.

Only one real model is available locally for this checkpoint: Qwen3-0.6B-Q8_0,
whose metadata is 5,932,855 bytes and descriptors end at 5,951,108. Synthetic tests
exercise larger-than-generic metadata, count limits, cumulative limits,
truncation and cancellation. No downloader is used and model quality is untested.

## Storage and compatibility

This is a development 0.10.0 schema update. Fresh test-owned registries are used;
old payloads are not silently defaulted or migrated. Historical M4-A evidence and
user storage remain unchanged. Deploying this to an existing user registry would
require a separately authorized explicit data decision, not read-time repair.
Limits: 64 artifacts, 32 distributions, 128 runtime definitions, 512 KiB registry
image, at most 48 owned files/48 aliases/32 modules per distribution. Existing
16 GiB managed-object budget and 30-second verification deadline remain.
