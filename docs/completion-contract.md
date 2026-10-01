# Completion contract

The completion scope of `zixcel-local-inference` is defined below.

## Requirements

1. Rust source knows no specific models, distributors, artifacts, revisions, digests or sizes.
2. Register multiple local-path and remote-endpoint sources.
3. Validate Ed25519-signed catalogs against closed schemas.
4. Reject rollback, equivocation and equal-priority conflicts fail-closed.
5. Build candidates dynamically from the current verified catalog generation.
6. Resolve only approved artifacts deterministically by format and runtime engine.
7. Externalize network transfers as Crowsi requests.
8. Verify delivered files by streaming SHA-256 and size before placement.
9. Revalidate status against the stored signed catalog generation.
10. Support idempotent installation, listing and explicitly confirmed deletion.
11. Register multiple provider-neutral runtime routes.
12. Do not equate placement with running inference.
13. Provide JSON CLI output consumable by Hatter.
14. Pass unit tests, CLI E2E, warning-free Clippy and release builds.

## Excluded from this package

- Hatter/HAT decisions and authority
- HTTP clients, proxies and credential custody
- Ollama, LM Studio or llama.cpp SDKs
- Model-specific prompts, inference jobs and long-term state
- Compatibility fallback for unsigned catalogs or unapproved artifacts

This boundary allows runtime implementations to change without recompiling Hatter Core or this package.