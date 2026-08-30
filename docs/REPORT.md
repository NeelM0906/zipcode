# Qwen3.8-27B serving report

Measured on 2026-08-27 on two NVIDIA RTX PRO 6000 Blackwell Max-Q
Workstation Edition GPUs (96 GB each).

## Selected deployment

- Official `Qwen/Qwen3.8-27B-FP8`, immutable revision
  `017b9c7af6b5689d5dd426a76e0bc077eb5ca20a`
- Two independent tensor-parallel-size-1 SGLang replicas behind the cache-aware
  SGLang router
- 1,000,000-token context using Qwen's static YaRN factor-4 recipe
- FP8 E4M3 KV cache and FP32 GDN recurrent state
- FlashInfer attention, Triton GDN, and GDN-aware radix caching
- MTP/EAGLE with 3 speculative steps, top-k 1, 4 draft tokens, and ReplaySSM
- Thinking and preserved thinking enabled by default; use
  `reasoning_effort: "xhigh"` for the maximum requested reasoning level
- Eight active requests per GPU and 256 additional queued requests

The independent-replica layout was chosen because this 27B FP8 checkpoint fits
on one 96 GB card, so it doubles aggregate serving capacity and avoids the
cross-GPU P2P/NCCL path that previously faulted on this host.

## Live measured short-request capacity

These are standardized synthetic `random-ids` runs through the dual-replica
router, not long-context or agent-quality evaluations. There were five requests
per concurrency slot. Input and output lengths varied in the generated sample.

| Active requests | Aggregate output tok/s | Median TTFT | Median TPOT | Approx. active-user tok/s |
|---:|---:|---:|---:|---:|
| 1 | 106.84 | 94 ms | 8.39 ms | 119.1 |
| 2 | 188.81 | 110 ms | 9.15 ms | 109.3 |
| 4 | 343.21 | 221 ms | 9.26 ms | 108.0 |
| 8 | 574.10 | 289 ms | 10.48 ms | 95.4 |
| 16 | 888.77 | 296 ms | 13.41 ms | 74.6 |

At concurrency 16, p95 TTFT was 1.116 seconds and p99 TPOT was 25.03 ms.
Sixteen simultaneous short/normal-context coding agents is the recommended
"fast" admission limit. More clients may queue, but should not be advertised as
simultaneously fast.

## Context-residency ceiling

Worker startup measured 1,488,992 and 1,492,464 cache tokens respectively.
These figures are memory ceilings, not latency guarantees:

| Maximum resident context per session | Total sessions across both GPUs |
|---:|---:|
| 1,000,000 | 2 (one per GPU) |
| 512,000 | 4 |
| 256,000 | 10 |
| 128,000 | 22 memory-resident; 16 actively scheduled by this config |
| 64,000 | More than 16 memory-resident; 16 actively scheduled |

Long prompts reduce throughput and increase TTFT. Prefix/radix caching can make
repeated agent repositories much cheaper, but a cold 1M prefill is inherently
heavy.

### Operational 1M proof

A direct request with 999,900 input token IDs and one requested decode token
completed successfully without OOM or admission failure. Wall time was
1,583.527 seconds (26 minutes 23.5 seconds), averaging about 631 input tokens/s
over the cold prefill. The synthetic prefix was flushed immediately afterward.
The compact response metadata is retained in `proof-1m.json`.

This proves that the configured limit is operational, but it also establishes a
hard product boundary: cold 1M requests are not interactive-fast on this machine.
For agentic coding, use repository/prompt prefix caching and keep normal active
windows well below 1M; reserve the two full-context slots for exceptional jobs.

## Functional validation

- OpenAI-compatible model discovery reports `max_model_len: 1000000`.
- A 999,900-input-token plus one-output-token request completed end to end.
- Thinking produced separate `reasoning_content` and reasoning-token usage.
- Structured tool calling produced a valid named function call and arguments.
- Vision correctly described a remote two-cat test image.

## Codex harness validation

Codex CLI 0.150.1 is configured through the `qwen38` profile and the local
Responses compatibility gateway on port 8002. A real stateless Codex loop was
validated through the model, not mocked:

- Qwen requested the Codex shell tool, Codex executed `pwd`, the gateway replayed
  the result, and the turn completed successfully.
- In a stronger coding-agent test, Qwen inspected a deliberately broken Python
  file, applied a file edit, ran the file, observed `FIXTURE_OK`, and completed
  with `CODING_AGENT_EDIT_OK`.
- The edit turn used 115,790 input tokens in total, of which 113,472 were cache
  hits, demonstrating that the repeated Codex/tool prefix was reused.
- After the public-edge cutover, the `codex-qwen` launcher completed an `xhigh`
  Responses turn with `PORT_SWAP_OK`.
- The public TLS endpoint rejected an unauthenticated health request with HTTP
  401, while an authenticated Responses request completed with
  `REMOTE_EDGE_OK`.

The local profile defaults to `xhigh` reasoning and an 850,000-token compaction
threshold. Non-coding apps/plugins and browser/computer tools are disabled for
this profile to keep the tool prefix small and avoid unsupported server-side
built-ins; the core shell and editing path is operational.

### Real-agent load and routing optimization

A correctness-gated workload subsequently ran complete Codex inspect/edit/test
trajectories at concurrency 1, 4, 8, and 16. Every one of 57 agents across the
initial matrix and the optimized C4/C8/C16 reruns passed its seven tests. The
initial C16 run exposed cache-aware router collapse onto one worker:
only eight requests ran, the queue reached 15, and median task latency was
132.12 seconds.

Reducing the cache-aware absolute imbalance threshold from its upstream default
of 64 to zero activated both eight-request replicas. On the identical C16 task,
the queue stayed at zero, peak generation increased from 622.70 to 1,211.29
tok/s, cache reuse rose from 87.03% to 96.20%, and median completion fell to
53.74 seconds. Full evidence and raw run paths are in `OPTIMIZATION_REPORT.md`.

Randomized exact-secret retrieval also passed at 32K, 128K, and 256K input
tokens, including needles at 10%, 50%, and 90% positions.

## Important quality tradeoffs

- Static YaRN factor 4 is necessary for the requested 1M ceiling and can slightly
  reduce quality on short contexts.
- FP8 KV is required to fit a genuine 1M session with the selected full-capability
  configuration on one 96 GB GPU. SGLang warns that this model does not supply
  calibrated FP8 KV scales, so it uses scale 1.0; this may reduce long-context
  accuracy. The weights themselves are the official FP8 checkpoint, not an
  unofficial low-bit quant.
- A prior local quality gate found the official FP8 weights close to BF16 on
  WikiText-2 (perplexity ratio 1.00326), but that is a narrow quality check rather
  than proof of universal BF16 equivalence.
