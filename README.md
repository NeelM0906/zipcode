<div align="center">
  <h1>ZIPCODE</h1>
  <p><strong>A private coding agent for your terminal.</strong></p>
  <p>
    Source-level Codex fork · GitHub device login · Self-hosted Qwen3.8 inference
  </p>

  [![CI](https://github.com/NeelM0906/zipcode/actions/workflows/ci.yml/badge.svg)](https://github.com/NeelM0906/zipcode/actions/workflows/ci.yml)
  [![Release](https://img.shields.io/github/v/release/NeelM0906/zipcode)](https://github.com/NeelM0906/zipcode/releases/latest)
  [![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
</div>

ZIPCODE is a source-level fork of the [OpenAI Codex CLI](https://github.com/openai/codex)
connected to a private, self-hosted model service. It keeps the terminal agent,
sandbox, approvals, tools, MCP support, and session workflow while using the
real Qwen model identities served by the ZIPCODE team.

## Install

macOS and Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/NeelM0906/zipcode/main/install.sh | sh
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/NeelM0906/zipcode/main/install.ps1 | iex
```

The installer downloads the release for your platform, verifies its SHA-256
checksum, and places the executables in your user path. Every release archive
also receives signed GitHub/Sigstore build provenance. See the
[installation guide](docs/install.md) for version pinning, manual installation,
and removal.

## Sign in and run

```bash
zip-code login
zip-code
```

`zip-code login` opens GitHub's device-authorization page. Your GitHub account
must have an active ZIPCODE invitation. No OpenAI API key or ChatGPT account is
used.

Run a non-interactive task with:

```bash
zip-code exec "explain this repository"
```

Switch models explicitly when a task needs the full context route:

```bash
zip-code -m Qwen/Qwen3.8-27B-FP8
```

The default is `Qwen/Qwen3.8-Flash-Next` with a 524K configured context. The
full route is `Qwen/Qwen3.8-27B-FP8` with a 1M configured context. These are
serving limits for the current deployment, not generic capability guarantees.

## What is included

- A Rust `zip-code` front end with isolated `~/.zipcode` state.
- A ZIPCODE-branded Codex runtime built from this repository's source.
- GitHub device authorization, invitation checks, 15-minute access JWTs,
  rotating refresh tokens, and immediate revocation.
- A two-model Responses-compatible gateway for Qwen3.8 Flash-Next and 27B FP8.
- Reproducible SGLang deployments, health checks, observability, benchmarks,
  and retained long-context evidence.
- Native release builds for Linux x86_64, macOS Apple Silicon and Intel, and
  Windows x86_64.

## Documentation

- [Installation](docs/install.md)
- [Authentication and invitations](docs/authentication.md)
- [Configuration](docs/config.md)
- [Sandbox and approvals](docs/sandbox.md)
- [Gateway and deployment report](docs/REPORT.md)
- [Measured optimization results](docs/OPTIMIZATION_REPORT.md)
- [Security boundary](SECURITY.md)
- [Contributing](docs/contributing.md)

## Build from source

Install a current Rust toolchain, then build both source-level executables:

```bash
git clone https://github.com/NeelM0906/zipcode.git
cd zipcode/codex-rs
ZIPCODE_GITHUB_CLIENT_ID=YOUR_PUBLIC_OAUTH_CLIENT_ID \
  cargo build --release --locked \
  -p codex-cli --bin codex \
  -p codex-zipcode-cli --bin zip-code \
  -p codex-code-mode-host --bin codex-code-mode-host
```

Install `target/release/zip-code` as `zip-code`, install
`target/release/codex` next to it as `zip-code-core`, and keep
`codex-code-mode-host` in the same directory. Release builds embed only the
public GitHub OAuth client ID; signing secrets remain on the gateway.

## Operate the private service

The service path is:

```text
zip-code
  -> GitHub device identity + ZIPCODE invitation
  -> authenticated HTTPS edge
  -> ZIPCODE model mux
     -> Qwen/Qwen3.8-Flash-Next
     -> Qwen/Qwen3.8-27B-FP8
```

Start with [the deployment guide](deploy/flash-next/README.md). Generate a
random `ZIPCODE_JWT_SECRET`, mount a private authentication-data directory,
invite the first GitHub login, and keep the origin bound to loopback behind the
HTTPS edge. Prompts, source code, GitHub tokens, and authorization headers are
not stored by the ZIPCODE control plane.

## Provenance and license

ZIPCODE is derived from OpenAI Codex and retains its upstream Git history,
Apache-2.0 license, and notices. ZIPCODE is an independent project and is not
affiliated with, endorsed by, or sponsored by OpenAI. See [NOTICE](NOTICE) and
[LICENSE](LICENSE).
