# Security

- Obtain model-specific values only from signed catalogs.
- Validate Ed25519 signatures, key IDs, closed JSON structures, identifiers, URLs, SHA-256 and sizes.
- Reject sequence rollback and differing content at the same sequence for one source.
- Do not silently choose between conflicting model definitions at equal priority.
- Zixcel never accesses the network; it accepts only regular files delivered by Crowsi.
- Reject symlinks, relative paths, parent references, unapproved artifacts, size/digest mismatches and overwriting existing state.
- Verify installed state against the exact signed catalog generation, not only its stored manifest.
- Placement alone does not establish inference readiness.
- Runtime routes are provider-neutral and retain neither credentials nor model SDKs.
- Deletion requires exact `MODEL@RELEASE` confirmation and targets only that directory.

`examples/sign_catalog` generates a one-use demonstration key for development. Production catalogs must be signed with offline keys held by the Zixcel release process.

## Common OSS reporting policy

# Security policy

## Reporting a vulnerability

Use this repository's Security tab and **Report a vulnerability** to submit a private
report to maintainers. Do not open a public issue or pull request containing exploit
details, credentials, customer data, or personal information. If private reporting
is unavailable, use GitHub's private security-support channel and request a private
reporting route before disclosing details.

Include affected versions, a minimal synthetic reproduction, expected and observed
behavior, and impact. Remove real secrets and identifying data. Maintainers assess
the report and coordinate a correction and disclosure. No response-time guarantee,
bounty, or support contract is implied.

## Supported versions

The current main branch is maintained during development. Released-version support
is stated in release notes; older releases are not implicitly supported. Do not infer
runtime safety from a source scan or a passing CI policy check. Dependencies,
deployments, history, and application-specific authorization require their own checks.
