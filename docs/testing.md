# Bounded process-failure tests

Run builds and tests sequentially with one job:

```sh
CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked --all-features -- --test-threads=1
```

Default acceptance includes actual child interruption and physical database
recovery. No recovery case is ignored. The writer runs against a fresh,
test-owned registry and acknowledges its protected state before interruption.
The parent checks explicit recovery, unchanged logical revision, original
receipt replay and the absence of implicit read-time repair.

`BackendProcessContext::attach` runs inside the final dedicated worker after
exec, before loading a native backend. It checks the live owner/process group,
arms Crowsi parent-death protection, sets finite per-worker limits and sets
PR_SET_DUMPABLE=0. Failure to establish or retain dump protection rejects work.
RLIMIT_CORE=0 alone is insufficient with a piped core collector such as WSL's.
No host sysctl, WSL crash collector, keyring or global resource setting changes.

Fault-injection signals target the test-owned child through a pidfd captured
at spawn. The helper exposes neither an arbitrary PID nor group-wide signalling.
It verifies waitable parentage, reaps the child and preserves its exact status.
Tests also verify that an unrelated child stays alive and parent dump policy
stays unchanged. The controlled abnormal-exit test releases its fault only
after a protection handshake and checks that the exit has no core-dump flag.

A non-dumpable child can also restrict parent/debugger access to /proc details.
Native integration must account for that observation boundary; do not silently
disable protection to enable tracing. The existing llama.cpp launch path is
not switched to the new backend context by these changes.

No model, native backend or large allocation is required for failure tests.
Allocation limits are read back and failure policy is exercised with small
bounded inputs, not actual host-memory exhaustion. Passing these tests is not
proof of physical OOM containment, model quality, GPU support or a long-term
absence of memory leaks. Confirm the boot ID and new collector/OOM records
before and after the controlled run; leave host configuration unchanged.

References: [process dumpability](https://man7.org/linux/man-pages/man2/PR_SET_DUMPABLE.2const.html),
[piped core limits](https://man7.org/linux/man-pages/man5/core.5.html),
[pidfd signals](https://docs.rs/rustix/1.1.5/rustix/process/fn.pidfd_send_signal.html).
