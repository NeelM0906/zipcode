# ZIPCODE

ZIPCODE is a private, self-hosted coding agent built on the Codex CLI harness
and locally served Qwen3.8 models. This repository captures the client,
compatibility gateways, deployment definitions, benchmarks, measured results,
and operator documentation used by the current pilot.

## Current architecture

```text
zip-code client
    -> authenticated HTTPS edge
    -> model mux
       -> Qwen3.8 Flash-Next NVFP4 (524K configured context)
       -> Qwen3.8-27B FP8 (1M configured context)
```

The default interactive route is Flash-Next. The full 27B route is intended for
exceptional large-context sessions. Public model names retain their real Qwen
repository identities; ZIPCODE is the client and service identity.

## Repository layout

- `client/` — isolated ZIPCODE launcher, installer, catalog, verification gate,
  and team/operator instructions.
- `gateway/` — Codex Responses compatibility gateway and two-model mux.
- `deploy/full/` — Qwen3.8-27B FP8 dual-replica deployment.
- `deploy/flash-next/` — Flash-Next GPU canary and public mux deployment.
- `deploy/observability/` — Prometheus, Grafana, alerts, and GPU exporter.
- `deploy/ngrok/` — credential-free edge policy and user-service templates.
- `benchmarks/` — real Codex agent and long-context correctness harnesses plus
  synthetic result records.
- `experiments/` — the retained prefill/decode-cadence experiment and raw
  machine-readable measurements.
- `docs/` — serving, optimization, and 1M-context evidence.
- `dist/` — the checked ZIPCODE teammate kit produced on 2026-08-28.

## Teammate installation

The operator sends `client/zip-code-setup.sh` and the pilot credential through
separate private channels. The teammate runs the installer once and then uses:

```bash
zip-code
ZIPCODE_MODEL=full zip-code
```

See [`client/README.md`](client/README.md) for the complete flow and
[`client/OPERATOR.md`](client/OPERATOR.md) for pilot operations.

## Serving

The committed Compose files are reproducibility records for the two validated
layouts. Copy the relevant `.env.example` to `.env`, set local checkpoint/cache
paths and GPU UUIDs, then review the manifest before starting it. Model weights,
container caches, credentials, local Codex state, and vendored SGLang source are
deliberately not stored in this repository.

The Flash-Next runtime depends on the external SGLang fork and immutable
revisions listed in [`DEPENDENCIES.md`](DEPENDENCIES.md).

## Measured status

The retained evidence includes:

- Qwen3.8-27B FP8 agent validation at concurrency 1, 4, 8, and 16.
- A router-balancing correction that reduced the measured C16 median task time
  from 132.12 s to 53.74 s.
- Exact-secret retrieval passes at 32K, 128K, and 256K inputs.
- An operational 999,900-input-token request proof.
- A decode-fair scheduling change that cut measured long-prompt p99 TPOT by
  88.4% to 96.8% in the tested cases.

Read [`docs/REPORT.md`](docs/REPORT.md) and
[`docs/OPTIMIZATION_REPORT.md`](docs/OPTIMIZATION_REPORT.md) for scope and
limitations. These measurements describe the recorded workstation and are not
generic capacity guarantees.

## Security boundary

This is private-pilot infrastructure, not a production multi-tenant control
plane. The current edge uses a shared bearer credential. Never commit the live
credential, a rendered ngrok policy, local `.env` files, session databases,
prompts, or source-bearing logs. See [`SECURITY.md`](SECURITY.md).
