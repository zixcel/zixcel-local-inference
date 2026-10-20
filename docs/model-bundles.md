# Model bundle integrity

`VerifiedModelBundle::verify` accepts an absolute existing directory, manifest
bytes, an independently selected SHA256, and `VerificationControl`. It verifies
the digest before parsing, then checks the complete directory inventory and every
file with streaming hashes. It never downloads, modifies or repairs a model.

The closed JSON manifest uses schema `zixcel://local-inference/model-bundle/v1`,
`format` (`openvino-ir` or `onnx`), immutable `sourceRevision` (40/64 lowercase hex),
`license`, and sorted `files` entries (`path`, `sha256`, `bytes`). Up to 64 flat
regular files and 16 GiB in total are accepted. Manifest bytes are limited to
64 KiB. Symlinks, traversal, hidden files, case collisions, extra and missing
files fail verification. Empty auxiliary files are allowed; the bundle cannot
be entirely empty. Hashing checks cancellation and the caller's deadline.

The revision and license are declarations, not a reviewed upstream identity or
legal approval. A digest supplied alongside an untrusted model is not an
independent trust decision. The owner must select the digest from an accepted
source, admit the backend distribution separately, and protect the model from
concurrent modification. `revalidate` observes current bytes; it does not make
writable files immutable or eliminate races after verification.

Existing signed single-file acquisition and physical llama.cpp launch remain
separate. Bundle verification alone does not enable a new physical backend.
