# Qwen3.8 Codex inference optimization report

Measured on 2026-08-27 with two RTX PRO 6000 Blackwell Max-Q 96 GB GPUs.

## Production finding: shared-prefix router collapse

The initial SGLang Model Gateway used the cache-aware defaults:

- absolute imbalance threshold: 64 active requests
- relative imbalance threshold: 1.5
- eight active requests per GPU

The absolute threshold was larger than the entire deployment's 16-request
capacity. Identical Codex system/tool prefixes therefore pinned almost every
turn to GPU0. During the initial C16 real-agent run, total active execution never
exceeded eight, the router queue peaked at 15, and GPU1 processed essentially no
agent tokens.

The live configuration now uses:

```text
--policy cache_aware
--balance-abs-threshold 0
--balance-rel-threshold 1.25
```

When both workers are idle, sequential turns still follow the best prefix match.
When concurrent load differs, the lighter replica is selected immediately. The
second replica quickly acquires the common Codex prefix and preserves locality.

## Before/after C16 evidence

All runs used the same independently verified seven-test repair task and `xhigh`
reasoning.

| Metric | Router defaults | Balanced router |
|---|---:|---:|
| Correct agents | 16/16 | 16/16 |
| Median task time | 132.12 s | 53.74 s |
| Peak active engine requests | 8 | 16 |
| Peak engine queue | 15 | 0 |
| Peak engine generation rate | 622.70 tok/s | 1,211.29 tok/s |
| Cumulative input cache reuse | 87.03% | 96.20% |
| Worker request split | 135 / approximately 0 | 85 / 70 |

The median improvement was 59.3%, and peak engine generation nearly doubled.
The optimized run contained one 256.95-second outlier that generated 22,052
tokens while repeatedly recovering from malformed patch attempts. The other 15
agents finished within 70.53 seconds. This is a model/tool trajectory-tail issue,
not queueing or GPU underutilization.

The raw comparison is retained in:

- `benchmarks/runs/codex-c16-n16-20260827T203758Z/summary.json`
- `benchmarks/runs/codex-c16-n16-20260827T204241Z/summary.json`

## Real Codex concurrency baseline

Initial correctness-gated runs before the router fix:

| Concurrency | Success | Median task | Aggregate Codex output tok/s | Input cache reuse |
|---:|---:|---:|---:|---:|
| 1 | 1/1 | 48.72 s | 93.22 | 96.84% |
| 4 | 4/4 | 48.01 s | 228.21 | 97.10% |
| 8 | 8/8 | 54.43 s | 431.30 | 95.91% |
| 16 | 16/16 | 132.12 s | 328.00 | 87.03% |

An optimized C8 rerun passed 8/8 and reduced median completion to 44.16 seconds.
Because agent paths and output length vary, these are first-pass operational
measurements rather than a statistically complete capacity study.

## Long-context retrieval and prefill

Randomized exact-secret retrieval passed at every tested length:

| Input | Needle position | Result | End-to-end time | Effective input tok/s |
|---:|---:|---:|---:|---:|
| 32,015 | 50% | pass | 4.82 s | 6,638 |
| 128,001 | 10% | pass | 41.84 s | 3,059 |
| 255,999 | 90% | pass | 139.58 s | 1,834 |

The earlier operational 1M allocation proof remains valid, but it is not a
retrieval-quality score. A full positional 512K/1M quality matrix remains to be
run during a dedicated maintenance window.

## Production finding: prefill-first decode starvation

A controlled GPU1 sweep compared 2K, 4K, and 8K chunks with fixed seeds, exact
tokenized random prompts, cache flushes, and identical request counts. Larger
chunks improved C1 prefill by only 2.6-3.1% at 32K and 4.7-6.5% at 128K. They
also reduced or left flat the C8 decode rate and raised concurrent TTFT, so 4K
and 8K were rejected.

The useful change was scheduler fairness, not chunk size. With the production
2K chunk retained, `--prefill-decode-interval 1` reduced p99 TPOT under 32K C4
prefill from 1,295.7 ms to 149.7 ms (-88.4%), and under 128K C2 prefill from
3,018.2 ms to 97.3 ms (-96.8%). Input throughput changed by -2.0% and -0.2%
respectively. Median TTFT for new long prompts rose by 48.9% and 31.5%; this is
an explicit decision to keep active coding streams responsive during prompt
bursts.

Correctness gates passed at 32K and 128K, the isolated candidate completed 4/4
real Codex tasks, and the rolled-out dual-replica service completed 8/8 real
Codex tasks at 258.8 aggregate output tok/s. Full evidence is in
`experiments/prefill-sweep/RESULTS.md`.

## Decisions after upstream review

- DFlash2 remains experimental. Current upstream reports include Qwen3.8 GDN
  state drift and cross-request context corruption under concurrency.
- HiCache remains disabled. Current hybrid GDN/Mamba host-tier eviction and
  loadback defects make it unsafe as a production capacity feature here.
- TP=2 and P/D disaggregation remain inappropriate on this two-GPU host: TP uses
  the previously faulting P2P path, while disaggregation would sacrifice the two
  independent full-context replicas.
- The next safe performance experiments are CUDA-graph-shape and
  speculative-width sweeps with fixed agent traces. Attention-backend changes
  remain gated by current SM120 correctness reports; chunked-prefill sizing has
  now been measured and closed at 2K.
