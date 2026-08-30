#!/usr/bin/env python3
"""Needle-retrieval and prefill-latency probe for the Qwen Responses endpoint."""

from __future__ import annotations

import argparse
import hashlib
import json
import secrets
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parent
DEFAULT_RUNS = ROOT / "runs"
FILLER = (
    "Routine archive record: service health was nominal, the scheduled audit "
    "completed, and no exceptional authorization was issued.\n"
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target-tokens", type=int, default=32_000)
    parser.add_argument("--needle-position", type=float, default=0.50)
    parser.add_argument("--endpoint", default="http://127.0.0.1:8002/v1/responses")
    parser.add_argument("--tokenize-endpoint", default="http://127.0.0.1:8010/tokenize")
    parser.add_argument("--model", default="Qwen/Qwen3.8-27B-FP8")
    parser.add_argument("--timeout", type=float, default=7200.0)
    parser.add_argument("--output-root", type=Path, default=DEFAULT_RUNS)
    args = parser.parse_args()
    if not 1_000 <= args.target_tokens <= 1_000_000:
        parser.error("--target-tokens must be between 1,000 and 1,000,000")
    if not 0.0 <= args.needle_position <= 1.0:
        parser.error("--needle-position must be between zero and one")
    return args


def post_json(url: str, payload: dict[str, Any], timeout: float) -> dict[str, Any]:
    request = urllib.request.Request(
        url,
        data=json.dumps(payload, separators=(",", ":")).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return json.load(response)
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"HTTP {exc.code} from {url}: {detail[:1000]}") from exc


def token_count(endpoint: str, prompt: str) -> int:
    value = post_json(endpoint, {"prompt": prompt}, timeout=600)
    return int(value["count"])


def make_prompt(repeats: int, needle_position: float, secret: str) -> str:
    before = round(repeats * needle_position)
    archive_nonce = hashlib.sha256(secret.encode()).hexdigest()
    needle = (
        "\nCRITICAL AUTHORIZATION RECORD: The exact authorization code is "
        f"{secret}. Retain this value for the final audit question.\n\n"
    )
    return (
        f"Archive run id: {archive_nonce}.\n"
        "Read the complete archive. At the end, answer the audit question with "
        "only the exact authorization code and no other text.\n\n"
        + FILLER * before
        + needle
        + FILLER * (repeats - before)
        + "\nAUDIT QUESTION: What is the exact authorization code?\n"
    )


def size_prompt(endpoint: str, target: int, position: float, secret: str) -> tuple[str, int, int]:
    one_hundred = token_count(endpoint, make_prompt(100, position, secret))
    one_hundred_one = token_count(endpoint, make_prompt(101, position, secret))
    marginal = max(1, one_hundred_one - one_hundred)
    fixed = one_hundred - 100 * marginal
    repeats = max(1, round((target - fixed) / marginal))
    prompt = make_prompt(repeats, position, secret)
    count = token_count(endpoint, prompt)
    for _ in range(3):
        error = target - count
        if abs(error) <= marginal:
            break
        repeats = max(1, repeats + round(error / marginal))
        prompt = make_prompt(repeats, position, secret)
        count = token_count(endpoint, prompt)
    return prompt, count, repeats


def output_text(response: dict[str, Any]) -> str:
    parts: list[str] = []
    for item in response.get("output") or []:
        if not isinstance(item, dict) or item.get("type") != "message":
            continue
        for content in item.get("content") or []:
            if isinstance(content, dict) and content.get("type") == "output_text":
                parts.append(str(content.get("text") or ""))
    return "".join(parts).strip()


def main() -> int:
    args = parse_args()
    secret = "Q38-" + secrets.token_hex(12).upper()
    prompt, tokenizer_tokens, repeats = size_prompt(
        args.tokenize_endpoint, args.target_tokens, args.needle_position, secret
    )
    payload = {
        "model": args.model,
        "input": prompt,
        "reasoning": {"effort": "medium"},
        "chat_template_kwargs": {"reasoning_effort": "xhigh"},
        "max_output_tokens": 2048,
        "stream": False,
    }
    started = time.monotonic()
    response = post_json(args.endpoint, payload, timeout=args.timeout)
    elapsed = time.monotonic() - started
    text = output_text(response)
    usage = response.get("usage") if isinstance(response.get("usage"), dict) else {}
    result = {
        "schema_version": 1,
        "timestamp_utc": datetime.now(timezone.utc).isoformat(),
        "model": response.get("model"),
        "target_tokens": args.target_tokens,
        "tokenizer_tokens": tokenizer_tokens,
        "reported_input_tokens": usage.get("input_tokens"),
        "reported_output_tokens": usage.get("output_tokens"),
        "needle_position": args.needle_position,
        "filler_repeats": repeats,
        "prompt_sha256": hashlib.sha256(prompt.encode()).hexdigest(),
        "elapsed_seconds": elapsed,
        "effective_input_tokens_per_second": (
            float(usage.get("input_tokens", tokenizer_tokens)) / elapsed if elapsed else 0.0
        ),
        "retrieval_passed": text == secret,
        "response_text": text,
        "response_status": response.get("status"),
    }
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    output_dir = args.output_root.resolve() / f"longctx-{args.target_tokens}-{stamp}"
    output_dir.mkdir(parents=True)
    output_file = output_dir / "result.json"
    output_file.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2))
    print(f"result={output_file}")
    return 0 if result["retrieval_passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
