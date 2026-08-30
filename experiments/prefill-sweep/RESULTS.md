# Prefill and decode-cadence experiment

Measured on 2026-08-27 against the pinned SGLang image and exact production
`Qwen/Qwen3.8-27B-FP8` checkpoint. GPU0 kept the public Codex service live while
GPU1 ran each candidate as an isolated TP=1 worker.

## Method

- Change one scheduler variable at a time.
- Flush the radix cache and run one warmup before every benchmark case.
- Use exact tokenized random prompts, fixed seeds, and identical request counts.
- Record decode throughput separately from 32K and 128K cold-prefill behavior.
- Reject a candidate unless every request completes and long-context retrieval
  plus real Codex tool use remain correct.

Raw SGLang JSONL results are under `results/`. Correctness artifacts and real
Codex summaries are retained beside them.

## Chunk-size sweep

| Workload | 2K baseline | 4K | 8K |
|---|---:|---:|---:|
| 32K input, C1 input tok/s | 4,965.9 | 5,093.5 (+2.6%) | 5,121.6 (+3.1%) |
| 128K input, C1 input tok/s | 2,882.5 | 3,017.4 (+4.7%) | 3,070.5 (+6.5%) |
| 2K in / 512 out, C8 output tok/s | 513.3 | 504.2 (-1.8%) | 510.9 (-0.5%) |
| 32K input, C4 median TTFT | 16.34 s | 19.29 s | 19.89 s |

Decision: keep 2K chunks. The larger chunks provide modest bulk-prefill gains,
not the requested massive speedup, while worsening interactive/concurrent
latency. The upstream 8K example that motivated the test is for the different
Qwen3.8-Flash-Next/QSA architecture and was not assumed transferable.

## Decode-fair cadence

The pinned SGLang scheduler normally selects prefill first whenever a prefill
batch can be formed. With speculative decoding enabled, mixed prefill/decode
batches are unavailable. `--prefill-decode-interval 1` is the minimum supported
cadence: after each prefill batch it guarantees one decode scheduling round.

| Workload | Cadence 0 | Cadence 1 | Delta |
|---|---:|---:|---:|
| 32K input, C4 input tok/s | 5,058.6 | 4,956.5 | -2.0% |
| 32K input, C4 median TTFT | 16.34 s | 24.32 s | +48.9% |
| 32K input, C4 p99 TPOT | 1,295.7 ms | 149.7 ms | -88.4% |
| 128K input, C2 input tok/s | 2,890.1 | 2,883.6 | -0.2% |
| 128K input, C2 median TTFT | 67.96 s | 89.39 s | +31.5% |
| 128K input, C2 p99 TPOT | 3,018.2 ms | 97.3 ms | -96.8% |
| 2K in / 512 out, C8 output tok/s | 513.3 | 517.1 | +0.7% |

This is a deliberate multi-user QoS trade: new long prompts wait longer during a
burst, while agents that are already generating no longer stall for seconds.
For an interactive coding-agent service, bounding stream stalls is more valuable
than a 0-2% bulk-prefill difference. Cadence 1 was promoted to both replicas.

## Correctness and operational gates

- Exact randomized retrieval passed at 32,828 and 128,042 reported input tokens.
- The isolated candidate passed 4/4 concurrent real Codex coding tasks at 251.7
  aggregate output tok/s on one GPU.
- The rolled-out two-replica service passed 8/8 concurrent real Codex coding
  tasks, processed 1,371,190 input tokens and 38,357 output tokens, and delivered
  258.8 aggregate Codex output tok/s.
- Both workers processed the post-rollout agent run; all Prometheus targets were
  healthy.
- Authenticated public `/v1/models` and `/v1/responses` checks returned HTTP 200
  and the exact expected response. Unauthenticated access returned HTTP 401.

The failed `214704Z` candidate-agent run is retained intentionally. It targeted
raw SGLang and proved that Codex `additional_tools` requires the compatibility
gateway. The corrected `214757Z` run used an isolated copy of the production
gateway and passed 4/4.

## Upstream evidence applied

- SGLang's official tuning guide says larger chunks favor prefill speed but cost
  memory and should be reduced to 4K/2K when needed.
- SGLang issue #32549 measures severe decode starvation under sustained
  chunked-prefill plus speculative decoding and asks for a minimum decode
  cadence—the exact failure mode measured here.
- SGLang issue #35537 describes a separate chunked-prefill admission flag bug on
  Qwen3.8. The live C8 correctness gate did not reproduce it, but it remains a
  tracked upgrade/patch item.
- Dynamic chunking is explicitly a pipeline-parallel feature in the pinned
  source, so it is inert for these TP1/PP1 replicas.
- Current DFlash2, HiCache hybrid-state, and SM120 NVFP4/TRTLLM reports still
  contain correctness blockers. None were mixed into this experiment.

Primary sources:

- https://github.com/sgl-project/sglang/blob/main/docs/advanced_features/hyperparameter_tuning.md
- https://github.com/sgl-project/sglang/issues/32549
- https://github.com/sgl-project/sglang/issues/35537
- https://github.com/sgl-project/sglang/issues/36701
- https://github.com/sgl-project/sglang/issues/31641
- https://docs.sglang.ai/backend/pd_disaggregation.html

## Next experiment

The next high-value test is CUDA-graph/decode-shape and speculative-width
profiling with fixed agent traces. Backend swaps, DFlash2, HiCache, TP2, and P/D
disaggregation stay blocked until their correctness or transport prerequisites
change.
