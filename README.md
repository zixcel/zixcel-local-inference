# zixcel-local-inference

Prepare and verify local model artifacts before a caller starts a managed inference runtime.

## What you can do

- Check artifact identity and placement requirements.
- Use declared acquisition and runtime boundaries.

## Current scope

Current verification and demonstration paths are not proof of a production model running. Real acquisition, inference and shutdown need separate integration evidence.

Package distribution is not activated by this documentation. Use the checked-in source and the declared dependency versions; published availability must be verified separately.

## Getting started

Install Rust 1.97 or newer and make the declared dependencies available. Use the configured private registry when a dependency is not distributed publicly. Run from this repository:

```sh
cargo test --locked
```

## Examples and interface details

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

## Documentation and source

[Interface reference](docs/interface-reference.md)

[Usage guide](docs/getting-started.md)

[Examples](examples) · [Schemas](schemas) · [Detailed documentation](docs) · [Implementation and public interfaces](src) · [Verification cases](tests) · [Contributing](CONTRIBUTING.md) · [Security reporting](SECURITY.md) · [License](LICENSE) · [Attribution notices](NOTICE)
