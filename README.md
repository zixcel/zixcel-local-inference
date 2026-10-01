# zixcel-local-inference

M4-A `verifyRuntime` is a pure, bounded observation of the existing static CPU
envelope and both exact managed artifacts. Missing/corrupt bytes or changed host
compatibility reject without rebinding the admitted definition. A successful
observation does not assert inference quality or a running process. Backend/model
capability is declared by the admitting owner; executable format and exact bytes
are verified, not arbitrary engine-specific behavior.

The [Runtime Distribution supplement](docs/runtime-distribution.md) adds exact
dynamic-library layout admission and bounded streaming GGUF metadata inspection.
Dynamic host loadability is not implied: `verifyRuntime` reports
`runtime-host-verification-required`; `verifyDistribution` rechecks immutable
owned bytes and dependency closure only.

The [M4-B runtime process contract](docs/runtime-process.md) adds explicit CPU
derivation and an external foreground owner. Reading definitions does not start it.
Process observations, including exact owner rejections, are separate from admission.

The [M4-C0 inference boundary](docs/inference-execution.md) adds bounded physical text
requests to that same owner. The provider accepts bounded text and an opaque caller context digest.
Semantic target admission and typed result interpretation belong to the caller.

A Zixcel-owned, model-neutral CLI for local inference artifact lifecycles. Model candidates and distribution locations are external catalog data. Version-pinned native-adapter compatibility rules are separate and constrained by audited exact artifact digests and rule revisions.

## Responsibilities

- Zixcel: signed catalogs, candidate resolution, acquisition plans, verified placement, status, deletion and runtime routes.
- Crowsi: network delivery of catalogs/artifacts and connection policy.
- External runtimes: model loading and inference processes.
- Hatter: route selection, HAT requests and use of inference results without authority.

Catalog `plan` and `source-request` produce closed Crowsi acquisition requests. Only local files delivered by Crowsi are verified and imported. Private backend communication also uses Crowsi's bounded transport; endpoints are not exposed in public APIs.

## Complete CLI demonstration

The following uses small non-model data to demonstrate signing, dynamic candidates, resolution, installation, status comparison and deletion.

```bash
cd $WONDERLAND_ROOT/ecosystem/providers/zixcel/services/zixcel/repositories/zixcel-local-inference

DEMO_DIR="$(mktemp -d)"

cargo run --example sign_catalog -- \
  "$PWD/examples/demo-catalog.json" \
  "$DEMO_DIR/source"
```

Use the `key-id` printed by `sign_catalog` as `DEMO_KEY_ID` below.

```bash
DEMO_KEY_ID='demo-local-lifecycle-demo-sequence-1'
BIN="$PWD/target/release/zixcel-local-inference"
STATE="$DEMO_DIR/state"
MODELS="$DEMO_DIR/models"

"$BIN" provision --state "$STATE"

"$BIN" source-add-file demo \
  --state "$STATE" \
  --catalog "$DEMO_DIR/source/catalog.signed.json" \
  --key-id "$DEMO_KEY_ID" \
  --public-key-file "$DEMO_DIR/source/catalog.public-key.txt"

"$BIN" refresh demo --state "$STATE"
"$BIN" candidates --state "$STATE"
"$BIN" plan demo-local-model --state "$STATE" --root "$MODELS"
"$BIN" install demo-local-model \
  --state "$STATE" \
  --root "$MODELS" \
  --artifact-file "$PWD/examples/demo-model.gguf"
"$BIN" status demo-local-model --state "$STATE" --root "$MODELS"
"$BIN" installed --state "$STATE" --root "$MODELS"
"$BIN" remove demo-local-model \
  --root "$MODELS" \
  --release demo-1 \
  --confirm demo-local-model@demo-1
```

The demo artifact is `lifecycle-demo-not-model-weights` and `inferenceReady` is `false`.

## Display selected candidates dynamically

The four selected candidates reside in replaceable catalog data, not Rust source.

```bash
cargo run --example sign_catalog -- \
  "$PWD/examples/accepted-candidates.catalog.json" \
  "$DEMO_DIR/candidates-source"
```

Register another source with its generated public key and key ID, then `refresh` to update candidates. Increase the catalog sequence and resign to update without recompiling the binary.

## Remote catalogs

```bash
zixcel-local-inference source-add-remote official \
  --state "$STATE" \
  --endpoint https://catalog.example/models.signed.json \
  --key-id RELEASE_KEY_ID \
  --public-key-file /path/to/release-public-key.txt

zixcel-local-inference source-request official --state "$STATE"
zixcel-local-inference refresh official --state "$STATE" --from /path/delivered-by-crowsi.json
```

The completion requirements are fixed in [docs/completion-contract.md](docs/completion-contract.md).
# Exact runtime admission (M4-A, development 0.10.0)

`CatalogStore` owns signed discovery and verified delivery, not exact runtime
admission. `RuntimeRouteRegistry` stores endpoint routes, not model identities.
Both expose pure `open_existing` and explicit `provision`; the old initializing
`open` entrypoint and ambiguous endpoint `RuntimeRegistry` name are removed.
The CLI uses `provision`, `route-add` and `route-list` for that existing catalog
utility. No read or HAT inspection provisions it.

The exact `RuntimeRegistry` admits immutable model/implementation bytes and
stable configurations. Its `RegistryCommand`/`RegistryReply` is shared with the
owner CLI (`registry --state ABS --request ABS`). This operation does not start
a process, choose a model, create a route, authorize an action or call inference.
Missing storage returns `registry-missing`, never a successful empty default.
After an unclean database-writer exit, `recover` explicitly asks the existing
Graph owner to recover its physical database. Reads never call it. It cannot
initialize a missing registry, repair an invalid canonical image, rematerialize
missing model bytes or change admitted identities.

Example explicit command documents:

```json
{"operation":"provision"}
```

```json
{"operation":"inspect"}
```

Artifact admission requires exact SHA-256, byte count, format and explicit public
provenance (publisher, license, immutable revision, HTTPS/URN source without
credentials/query/fragment). These are preserved owner assertions; merely
supplying them is **not** signature verification. Catalog signatures retain
their separate existing verification path. No sample fixture is a recommended
model or runtime.

Supported envelopes are little-endian GGUF v3 (bounded metadata and
non-overlapping supported tensor extents) and single-file static ELF64 for Linux.
The static profile still rejects interpreters/shared libraries. Dynamic ELF
components require a separately admitted exact RuntimeDistribution: an executable
digest alone cannot identify their dependency closure. Admission validates format and
target, not inference quality or successful loading. Implementation compatibility
is explicitly declared; process/liveness and actual model execution remain M4-B
and subsequent work. Provider URLs remain route data, not implementation IDs.

Model configuration fixes `contextTokens` and `batchTokens`; runtime configuration
fixes `threads` and `memoryBytes`. Unknown/duplicate fields reject. Prompts,
temperature, scheduling, worker generations and account credentials are excluded.
Configuration field order does not change identity; changing meaningful values
does. A memory limit is an admitted setting, not an observed RSS guarantee.

Zixcel Graph owns atomic CAS/commit/receipt persistence. Identical requests replay
the original receipt even after their delivered source file disappears. New
requests require an exact `expectedRevision`. Source paths do not become IDs.
Verified bytes are streamed into no-overwrite managed objects; old references
never rebind when a source path changes. No deletion API for admitted objects is
provided. Discovery installation removal cannot delete these managed objects.

Bounds: 64 artifacts, 32 distributions, 128 runtime definitions (at most 256 inline config values),
512 KiB current registry payload, Graph's 64 MiB retained commit payload bound,
8 GiB per artifact, 16 GiB managed object bytes including unfinished copies,
128 physical objects, 2 MiB generic/static inspection and a 64 KiB stream buffer.
GGUF uses a separate 64 MiB cumulative metadata/descriptor budget, not a metadata
allocation; individual fields/counts remain bounded. Admission
has a cooperative 30-second maximum verification budget and explicit cancellation;
one owner file-lock lane serializes mutations across processes. Storage and
verification failures are not silently repaired or retried with another model.

## Package integration

The package is an independently consumable unit. Callers reference its documented
interface through a versioned dependency and own application-specific composition
and integration.

## Inference integration

Physical IPC v2 carries bounded text, runtime/process references, an opaque context digest and finite execution options. The provider checks exact process incarnation, replay identity, cancellation, output limits and the original owner execution budget. Callers own application-specific admission and interpretation. Callers retain their existing repository fences before submission and before result admission. Old IPC requests are rejected; runtime restart and state reset are explicit owner operations.
