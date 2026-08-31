# Contributing

ZIPCODE accepts focused bug reports, design discussions, documentation fixes,
and pull requests through the
[NeelM0906/zipcode repository](https://github.com/NeelM0906/zipcode).

Before opening an issue, search the existing issues and verify the problem on
the latest ZIPCODE release. Never include access tokens, private URLs, prompts,
source code from another repository, or unredacted agent logs.

## Development

ZIPCODE retains the Codex Rust workspace and its development conventions. From
the repository root:

```bash
just fmt
just test -p codex-zipcode-cli
python3 -m unittest gateway.tests.test_auth -v
```

For Rust changes, keep the scope narrow, add tests for new behavior, and run
the affected package suite. Gateway changes should preserve the public health
routes, require authentication for model and inference routes, and never log
credentials or prompt bodies.

## Pull requests

Describe the behavior being changed, why it is needed, and the verification
you performed. Keep unrelated refactors in separate pull requests. Changes
derived from upstream Codex must remain compatible with the Apache-2.0 license
and preserve required attribution.

Security vulnerabilities should be reported according to
[SECURITY.md](../SECURITY.md), not through a public issue.
