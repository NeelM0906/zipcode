# Qwen Codex serving benchmarks

These benchmarks measure complete agent behavior, not only token generation.
Every Codex agent receives an isolated copy of a defective Python module, must
inspect it, edit it, run seven tests, and emit `CODEX_AGENT_BENCH_OK`. The
harness independently reruns the test suite before counting a success.

Run one concurrency point:

```bash
./codex_agent_bench.py --concurrency 8 --tasks 8
```

Run the complete admission matrix:

```bash
./run_codex_matrix.sh
```

Each run writes a `summary.json` plus raw Codex JSONL, stderr, and independent
test logs below `runs/`. Important fields include success rate, wall time,
per-task latency, Codex token usage, prefix-cache reuse, and deltas from both
SGLang workers and the compatibility gateway.

The benchmark collector follows the live two-replica deployment: workers on
ports 8010 and 8020, Codex gateways on 8012 and 8022, and the router on 29000.

Because the profile uses sampling and `xhigh` reasoning, trajectory length is
stochastic. Repeat each concurrency point before making capacity decisions;
compare correctness and median latency separately from makespan and aggregate
tokens/s. A single agent that repeatedly repairs malformed patches can dominate
the latter two figures.

## Long-context retrieval

The long-context probe generates a fresh random secret, hides it at a selected
position in a sized archive, and requires an exact final answer:

```bash
./long_context_probe.py --target-tokens 32000 --needle-position 0.50
./long_context_probe.py --target-tokens 128000 --needle-position 0.10
./long_context_probe.py --target-tokens 256000 --needle-position 0.90
```

The prompt itself is not retained; the result records only its SHA-256 hash,
token counts, timing, position, response, and correctness. A 1M run is opt-in
because a cold prefill takes tens of minutes on this host.
