# Qwen3.8-27B dual-replica server

This deployment serves the immutable official `Qwen/Qwen3.8-27B-FP8` revision
`017b9c7af6b5689d5dd426a76e0bc077eb5ca20a` from the read-only local checkpoint.
The server image is pinned to SGLang commit `5f55db35e926d50676f75b812640ea2410b0fe0e`
and image digest `sha256:616a3e97f45191af975896cfa644279096cb31bd408a071c2e99ca7209c3cafe`.

- Codex compatibility gateway: `http://127.0.0.1:8012/v1`
- Public two-model mux when the Flash-Next overlay is active: `http://127.0.0.1:8002/v1`
- Internal raw router: `http://127.0.0.1:8003/v1`
- GPU 0 worker: `http://127.0.0.1:8010/v1`
- GPU 1 worker: `http://127.0.0.1:8011/v1`
- Served model name: `Qwen/Qwen3.8-27B-FP8`
- Two independent TP=1 replicas; no cross-GPU P2P or NCCL transport
- 1,000,000-token maximum context via the model card's static YaRN factor-4 recipe
- Thinking and preserved thinking enabled; request `reasoning_effort: xhigh` for maximum reasoning
- Official FP8 weights, FP8 E4M3 KV cache, FP32 GDN recurrent state
- Built-in MTP/EAGLE (3 steps, top-k 1, 4 draft tokens) with ReplaySSM
- Cache-aware routing with immediate two-replica load balancing and GDN-aware
  radix caching; 8 active requests per GPU
- One guaranteed decode round after every prefill chunk to bound multi-user
  coding-stream stalls (`--prefill-decode-interval 1`)

Start, inspect, or stop:

```bash
./start.sh
./status.sh
docker compose logs -f gpu0 gpu1 router codex-gateway
./stop.sh
```

Local monitoring and real-agent benchmarks:

```bash
docker compose --project-directory ../observability -f ../observability/compose.yaml up -d
../observability/status.sh
../../benchmarks/codex_agent_bench.py --concurrency 8 --tasks 8
```

Grafana is available locally at
`http://127.0.0.1:3000/d/qwen38-codex-serving/qwen3-8-codex-serving`.
See `../../docs/OPTIMIZATION_REPORT.md` for the measured router fix and
`../../experiments/prefill-sweep/RESULTS.md` for the promoted scheduler-cadence result.

## Use from Codex

The historical local Codex profile was stored in `~/.codex/qwen38.config.toml`.
The supported team launcher is now the isolated client under `../../client/`.

```bash
../../client/zip-code
ZIPCODE_MODEL=full ../../client/zip-code
```

The profile selects `Qwen/Qwen3.8-27B-FP8`, a 1,000,000-token context window,
an 850,000-token auto-compaction threshold, and `xhigh` reasoning. Core Codex
shell/edit tools are enabled. Apps, browser/computer tools, plugins, Railway MCP,
and multi-agent tools are disabled in this performance profile so hundreds of
irrelevant schemas are not inserted into every coding turn.

Codex 0.150 uses newer Responses API constructs that the pinned SGLang frontend
does not accept directly. The gateway performs these lossless coding-path
translations:

- Responses-Lite `additional_tools` items to ordinary top-level tools
- namespace members to SGLang-safe flat names and back
- custom/freeform tool calls to JSON function calls and back
- custom tool outputs to function outputs
- stateless assistant replay normalization
- `xhigh`/`max` reasoning preservation across the router/model enum mismatch
- Codex model-catalog metadata while preserving ordinary `/v1/models` behavior

The gateway logs request IDs and tool structure, never prompt text, tool
arguments, tool output, or authorization headers.

### Remote Codex users

The authenticated public endpoint is `https://notzipcode.ngrok.io/v1`.
The preferred onboarding path is the isolated kit in `team/`; it does not touch
a user's normal Codex configuration:

```bash
cd ../../client
./zip-code-setup.sh
zip-code
```

Give authorized users the bearer credential through a private channel; do not
put it in TOML. For a manual installation, they may instead add this provider to
`~/.codex/config.toml`:

```toml
[model_providers.qwen38_remote]
name = "Qwen3.8-27B remote"
base_url = "https://notzipcode.ngrok.io/v1"
env_key = "ZIPCODE_API_KEY" # gitleaks:allow -- this is a variable name, not a credential
wire_api = "responses"
request_max_retries = 2
stream_max_retries = 2
stream_idle_timeout_ms = 1800000
```

Then install the supplied fast coding profile and launch it:

```bash
cp qwen38-remote.config.toml.example ~/.codex/qwen38-remote.config.toml
export QWEN38_API_KEY='credential-received-privately'
codex --profile qwen38-remote
```

The existing tunnel policy rejects missing or incorrect bearer credentials.

Example request:

```bash
curl http://127.0.0.1:8002/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "Qwen/Qwen3.8-27B-FP8",
    "messages": [{"role": "user", "content": "Write a robust Python retry helper."}],
    "reasoning_effort": "xhigh",
    "temperature": 1.0,
    "top_p": 0.95,
    "top_k": 20,
    "max_tokens": 32768,
    "stream": true
  }'
```

The 1M limit is an admission ceiling, not a promise that every concurrent user can
hold 1M tokens. The live token-pool size printed by each worker at startup is the
capacity authority. Static YaRN factor 4 can slightly reduce short-context quality;
it is enabled because this deployment explicitly requires 1M support.
