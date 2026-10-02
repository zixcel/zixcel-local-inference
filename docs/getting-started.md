# Using zixcel-local-inference

Prepare and verify local model artifacts before a caller starts a managed inference runtime.

## Before you start

Current verification and demonstration paths are not proof of a production model running. Real acquisition, inference and shutdown need separate integration evidence.

## First steps

Run from the repository root:

```sh
cargo test --locked
```

## How to assess the result

- Check artifact identity and placement requirements.
- Use declared acquisition and runtime boundaries.

A passing source-level check establishes only what that check observes. Keep missing configuration, unavailable services and unverified deployment paths visible.

## Continue reading

[Repository overview](../README.md)
