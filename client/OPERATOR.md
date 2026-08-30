# ZIPCODE pilot operator runbook

## Onboard a tester

1. Send `zip-code-setup.sh` or the versioned ZIPCODE kit archive through the
   normal team file-sharing channel.
2. Send the bearer credential separately through the team's password manager or
   another private, expiring channel. Never embed it in the installer or archive.
3. Tell the tester: run setup once, open a new terminal, then type `zip-code`.
4. If verification is requested, ask for only the `PASS` lines—never credentials
   or verbose environment dumps.
5. Begin with 2–4 active Qwen3.8 Flash-Next users. The 1M model is an exceptional-job
   lane, not the default.

## Observe the service

```bash
cd /path/to/zipcode
./deploy/full/status.sh
./deploy/observability/status.sh
docker compose --project-directory deploy/full logs --since 15m router codex-gateway

./deploy/flash-next/status.sh
docker compose --project-directory deploy/flash-next logs --since 15m model codex-gateway model-mux
```

Grafana is local at
`http://127.0.0.1:3000/d/qwen38-codex-serving/qwen3-8-codex-serving`.
The compatibility gateways log request IDs and tool structure, not prompts,
tool arguments, tool outputs, or authorization headers.

## Product-to-runtime mapping

| Public ZIPCODE model | Runtime model | GPU | Qualified context |
|---|---|---:|---:|
| `Qwen/Qwen3.8-Flash-Next` | `qwen-codex-flash-next` / Qwen3.8 Flash-Next NVFP4 | 1 | 524,288 |
| `Qwen/Qwen3.8-27B-FP8` | `Qwen/Qwen3.8-27B-FP8` | 0 | 1,000,000 |

The mux rewrites the public Flash repository name to the internal serving name before forwarding. Legacy `zipcode-flash` and `zipcode-full` IDs remain accepted temporarily but are no longer advertised by `/model`.

## Pilot security and capacity boundary

The ngrok edge currently uses one shared pilot bearer. Before broad rollout,
replace it with per-user or per-service credentials, revocation, rate limits,
request accounting, and an audit identity that does not log source code.

- Qwen3.8 Flash-Next admission: four active requests. The measured four-agent Codex
  gate passed 4/4 at 316.12 aggregate output tokens/s.
- Qwen3.8-27B FP8 admission: eight active requests through the GPU 0 router, but a
  full 1M-resident job should be treated as a single exceptional session.
- Mixed-model load still needs a sustained soak test before raising pilot size.
