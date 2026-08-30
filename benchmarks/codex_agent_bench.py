#!/usr/bin/env python3
"""Concurrent, correctness-gated benchmark for the local Codex/Qwen service."""

from __future__ import annotations

import argparse
import asyncio
import json
import math
import os
import shutil
import statistics
import sys
import time
import urllib.request
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parent
FIXTURE = ROOT / "fixture" / "agent_fixture"
DEFAULT_RUNS = ROOT / "runs"
METRIC_URLS = (
    "http://127.0.0.1:8010/metrics",
    "http://127.0.0.1:8011/metrics",
    "http://127.0.0.1:8002/metrics",
    "http://127.0.0.1:29000/metrics",
)
COUNTER_PREFIXES = (
    "sglang:prompt_tokens_total",
    "sglang:generation_tokens_total",
    "sglang:cached_tokens_total",
    "sglang:prefill_effective_tokens_total",
    "sglang:num_requests_total",
    "sglang:num_aborted_requests_total",
    "sglang:spec_verify_calls_total",
    "sglang:cuda_graph_passes_total",
    "sglang:time_to_first_token_seconds",
    "sglang:e2e_request_latency_seconds",
    "sglang:queue_time_seconds",
    "qwen_codex_gateway_requests_total",
    "qwen_codex_gateway_transformations_total",
    "smg_router_requests_total",
    "smg_router_upstream_responses_total",
    "smg_worker_selection_total",
)


@dataclass
class TaskResult:
    task_id: int
    returncode: int
    elapsed_seconds: float
    timed_out: bool
    marker_seen: bool
    tests_passed: bool
    input_tokens: int
    cached_input_tokens: int
    output_tokens: int
    reasoning_output_tokens: int
    workdir: str
    stdout_file: str
    stderr_file: str
    test_output_file: str

    @property
    def successful(self) -> bool:
        return self.returncode == 0 and not self.timed_out and self.marker_seen and self.tests_passed


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--concurrency", type=int, default=1)
    parser.add_argument("--tasks", type=int, help="Total agent tasks; defaults to concurrency")
    parser.add_argument("--timeout", type=float, default=900.0)
    parser.add_argument("--codex-bin", default=os.environ.get("ZIPCODE_BIN", "zip-code"))
    parser.add_argument("--model-label", default="Qwen/Qwen3.8-27B-FP8")
    parser.add_argument("--sandbox", default="danger-full-access")
    parser.add_argument("--output-root", type=Path, default=DEFAULT_RUNS)
    parser.add_argument("--keep-workdirs", action="store_true")
    args = parser.parse_args()
    if args.concurrency < 1:
        parser.error("--concurrency must be positive")
    if args.tasks is None:
        args.tasks = args.concurrency
    if args.tasks < 1:
        parser.error("--tasks must be positive")
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    return args


def fetch_metrics() -> dict[str, float]:
    snapshot: dict[str, float] = {}
    for index, url in enumerate(METRIC_URLS):
        try:
            with urllib.request.urlopen(url, timeout=5) as response:
                body = response.read().decode("utf-8", errors="replace")
        except OSError:
            continue
        for line in body.splitlines():
            if not line or line.startswith("#") or " " not in line:
                continue
            name, raw_value = line.rsplit(" ", 1)
            if not name.startswith(COUNTER_PREFIXES):
                continue
            try:
                value = float(raw_value)
            except ValueError:
                continue
            snapshot[f"target{index}:{name}"] = value
    return snapshot


def metric_delta(before: dict[str, float], after: dict[str, float]) -> dict[str, float]:
    return {
        key: value - before.get(key, 0.0)
        for key, value in after.items()
        if value - before.get(key, 0.0) != 0.0
    }


def extract_usage(stdout: str) -> dict[str, int]:
    usage: dict[str, int] = {}
    for line in stdout.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("type") != "turn.completed" or not isinstance(event.get("usage"), dict):
            continue
        usage = {
            key: int(value)
            for key, value in event["usage"].items()
            if isinstance(value, (int, float))
        }
    return usage


async def run_tests(workdir: Path, output_file: Path) -> bool:
    process = await asyncio.create_subprocess_exec(
        sys.executable,
        "-m",
        "unittest",
        "-v",
        cwd=workdir,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.STDOUT,
    )
    stdout, _ = await process.communicate()
    output_file.write_bytes(stdout)
    return process.returncode == 0


async def run_task(
    task_id: int,
    semaphore: asyncio.Semaphore,
    run_dir: Path,
    args: argparse.Namespace,
) -> TaskResult:
    workdir = run_dir / "workdirs" / f"task-{task_id:03d}"
    shutil.copytree(FIXTURE, workdir)
    stdout_file = run_dir / "raw" / f"task-{task_id:03d}.stdout.jsonl"
    stderr_file = run_dir / "raw" / f"task-{task_id:03d}.stderr.log"
    test_output_file = run_dir / "raw" / f"task-{task_id:03d}.tests.log"
    prompt = (workdir / "TASK.md").read_text(encoding="utf-8")
    prompt += "\nWork only inside the current directory. Inspect the implementation and tests before editing.\n"

    command = (
        args.codex_bin,
        "exec",
        "--json",
        "--sandbox",
        args.sandbox,
        "--skip-git-repo-check",
        "-C",
        str(workdir),
        prompt,
    )
    started = time.monotonic()
    timed_out = False
    async with semaphore:
        process = await asyncio.create_subprocess_exec(
            *command,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env=os.environ.copy(),
        )
        try:
            stdout_bytes, stderr_bytes = await asyncio.wait_for(
                process.communicate(), timeout=args.timeout
            )
        except asyncio.TimeoutError:
            timed_out = True
            process.kill()
            stdout_bytes, stderr_bytes = await process.communicate()
    elapsed = time.monotonic() - started
    stdout_file.write_bytes(stdout_bytes)
    stderr_file.write_bytes(stderr_bytes)
    stdout = stdout_bytes.decode("utf-8", errors="replace")
    usage = extract_usage(stdout)
    tests_passed = await run_tests(workdir, test_output_file)

    return TaskResult(
        task_id=task_id,
        returncode=process.returncode if process.returncode is not None else -1,
        elapsed_seconds=elapsed,
        timed_out=timed_out,
        marker_seen="CODEX_AGENT_BENCH_OK" in stdout,
        tests_passed=tests_passed,
        input_tokens=usage.get("input_tokens", 0),
        cached_input_tokens=usage.get("cached_input_tokens", 0),
        output_tokens=usage.get("output_tokens", 0),
        reasoning_output_tokens=usage.get("reasoning_output_tokens", 0),
        workdir=str(workdir),
        stdout_file=str(stdout_file),
        stderr_file=str(stderr_file),
        test_output_file=str(test_output_file),
    )


async def async_main(args: argparse.Namespace) -> int:
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    run_dir = args.output_root.resolve() / f"codex-c{args.concurrency}-n{args.tasks}-{stamp}"
    (run_dir / "raw").mkdir(parents=True)
    (run_dir / "workdirs").mkdir()
    before = fetch_metrics()
    semaphore = asyncio.Semaphore(args.concurrency)
    wall_started = time.monotonic()
    results = await asyncio.gather(
        *(run_task(task_id, semaphore, run_dir, args) for task_id in range(1, args.tasks + 1))
    )
    wall_seconds = time.monotonic() - wall_started
    after = fetch_metrics()

    successful = sum(result.successful for result in results)
    output_tokens = sum(result.output_tokens for result in results)
    input_tokens = sum(result.input_tokens for result in results)
    cached_tokens = sum(result.cached_input_tokens for result in results)
    summary: dict[str, Any] = {
        "schema_version": 1,
        "timestamp_utc": stamp,
        "model": args.model_label,
        "codex_binary": args.codex_bin,
        "concurrency": args.concurrency,
        "tasks": args.tasks,
        "wall_seconds": wall_seconds,
        "successful_tasks": successful,
        "success_rate": successful / len(results),
        "aggregate_input_tokens": input_tokens,
        "aggregate_cached_input_tokens": cached_tokens,
        "aggregate_output_tokens": output_tokens,
        "codex_output_tokens_per_second": output_tokens / wall_seconds if wall_seconds else 0.0,
        "median_task_seconds": statistics.median(result.elapsed_seconds for result in results),
        "p95_task_seconds": sorted(result.elapsed_seconds for result in results)[
            max(0, math.ceil(0.95 * len(results)) - 1)
        ],
        "metrics_delta": metric_delta(before, after),
        "results": [asdict(result) | {"successful": result.successful} for result in results],
    }
    (run_dir / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({key: value for key, value in summary.items() if key not in {"metrics_delta", "results"}}, indent=2))
    print(f"summary={run_dir / 'summary.json'}")

    if not args.keep_workdirs:
        shutil.rmtree(run_dir / "workdirs")
    return 0 if successful == len(results) else 1


def main() -> int:
    args = parse_args()
    if not FIXTURE.is_dir():
        raise SystemExit(f"fixture not found: {FIXTURE}")
    return asyncio.run(async_main(args))


if __name__ == "__main__":
    raise SystemExit(main())
