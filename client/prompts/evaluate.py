#!/usr/bin/env python3
"""Bounded POSIX installed-CLI smoke evaluation. No automatic promotion."""

import argparse
import asyncio
from collections import Counter
import json
import hashlib
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

from eval_cases import CASES


def summarize(events: list[dict]) -> dict:
    items = [event["item"] for event in events if event.get("type") == "item.completed"]
    commands = [item for item in items if item.get("type") == "command_execution"]
    counts = Counter(item.get("command", "") for item in commands)
    usage = [
        event["usage"]
        for event in events
        if event.get("type") == "turn.completed" and "usage" in event
    ]
    return {
        "turn_completed": any(
            event.get("type") == "turn.completed" for event in events
        ),
        "usage": (
            {
                key: (
                    sum(entry[key] for entry in usage)
                    if all(isinstance(entry.get(key), int) for entry in usage)
                    else None
                )
                for key in set().union(*(entry.keys() for entry in usage))
            }
            if usage
            else None
        ),
        "command_count": len(commands),
        "repeated_commands": sum(count - 1 for count in counts.values()),
        "commands": [
            {key: item.get(key) for key in ("command", "exit_code")}
            for item in commands
        ],
        "final": "\n".join(
            item.get("text", "")
            for item in items
            if item.get("type") == "agent_message"
        ),
    }


async def run_bounded(
    argv: list[str], cwd: Path, timeout: float
) -> tuple[int, bytes, bytes, str | None]:
    if os.name != "posix":
        raise RuntimeError(
            "Live evaluation requires POSIX process-group cleanup; Windows is not supported yet"
        )
    environment = {
        key: value
        for key, value in os.environ.items()
        if key
        in {
            "PATH",
            "HOME",
            "USER",
            "LOGNAME",
            "TMPDIR",
            "LANG",
            "SYSTEMROOT",
            "WINDIR",
            "USERPROFILE",
            "LOCALAPPDATA",
        }
    }
    environment["ZIPCODE_DISABLE_TRACE_UPLOAD"] = "1"
    process = await asyncio.create_subprocess_exec(
        *argv,
        cwd=cwd,
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        start_new_session=True,
    )
    remaining = 8 * 1024 * 1024
    captured = [bytearray(), bytearray()]
    overflow = asyncio.Event()

    async def read(stream, index):
        nonlocal remaining
        while chunk := await stream.read(8192):
            take = min(remaining, len(chunk))
            captured[index].extend(chunk[:take])
            remaining -= take
            if take < len(chunk):
                overflow.set()

    reason = None
    collection = asyncio.gather(
        read(process.stdout, 0), read(process.stderr, 1), process.wait()
    )
    limit = asyncio.create_task(overflow.wait())
    try:
        done, _ = await asyncio.wait(
            {collection, limit}, timeout=timeout, return_when=asyncio.FIRST_COMPLETED
        )
        if overflow.is_set():
            reason = "output_limit"
        elif collection not in done:
            reason = "timeout"
        else:
            await collection
    finally:
        limit.cancel()
        await asyncio.gather(limit, return_exceptions=True)
        # Kill the entire owned process group, including children holding pipes open.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            await asyncio.wait_for(collection, timeout=5)
        except TimeoutError:
            collection.cancel()
            await asyncio.gather(collection, return_exceptions=True)
            reason = reason or "cleanup_timeout"
    return process.returncode, bytes(captured[0]), bytes(captured[1]), reason


async def evaluate(args) -> None:
    args.output.mkdir(mode=0o700, parents=True)
    baseline_catalog = json.loads(args.catalog.read_text())
    model = next(
        entry for entry in baseline_catalog["models"] if entry["slug"] == args.model
    )
    messages = model.get("model_messages") or {}
    baseline = messages.get("instructions_template") or model["base_instructions"]
    if messages.get("instructions_variables") is not None:
        raise ValueError(
            "Baseline uses template variables; capture the rendered prompt first"
        )
    baseline_file = args.output / "baseline.md"
    baseline_file.write_text(baseline)
    candidate_text = args.candidate.read_text(encoding="utf-8")
    candidate_file = args.output / "candidate.md"
    candidate_file.write_text(candidate_text, encoding="utf-8")
    (args.output / "manifest.json").write_text(
        json.dumps(
            {
                "baseline_sha256": hashlib.sha256(baseline.encode()).hexdigest(),
                "candidate_sha256": hashlib.sha256(candidate_text.encode()).hexdigest(),
                "model": args.model,
                "reasoning_effort": args.effort,
                "grader_sha256": hashlib.sha256(
                    json.dumps(CASES, sort_keys=True).encode()
                ).hexdigest(),
                "skills_catalog": "disabled for both variants to isolate base-prompt behavior",
                "web_search": "disabled for synthetic offline tasks",
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    results = []
    for case_name in args.cases:
        case = CASES[case_name]
        for variant, prompt_file in (
            ("baseline", baseline_file),
            ("candidate", candidate_file),
        ):
            if variant not in args.variants:
                continue
            directory = args.output / f"{case_name}-{variant}"
            directory.mkdir()
            for name, content in case["files"].items():
                (directory / name).write_text(content)
            if case["check"] is not None:
                (directory / "test_app.py").write_text(
                    case.get("visible_check", case["check"])
                )
            instructions = "Use python3 test_app.py for local checks when present. Do not change the existing assertions.\n"
            (directory / "AGENTS.md").write_text(instructions)
            argv = [
                str(args.binary),
                "-c",
                f"model_instructions_file={json.dumps(str(prompt_file))}",
                "-c",
                'web_search="disabled"',
                "-c",
                "skills.include_instructions=false",
                "-c",
                'approval_policy="never"',
                "-c",
                f"model_reasoning_effort={json.dumps(args.effort)}",
                "exec",
                "--ephemeral",
                "--skip-git-repo-check",
                "--json",
                "-m",
                args.model,
                "-s",
                "workspace-write",
                "-C",
                str(directory),
                case["prompt"],
            ]
            started = time.monotonic()
            code, stdout, stderr, stopped = await run_bounded(
                argv, directory, args.timeout
            )
            # Logs remain outside the agent's workspace and contain synthetic task data only.
            (args.output / f"{case_name}-{variant}.jsonl").write_bytes(stdout)
            (args.output / f"{case_name}-{variant}.stderr").write_bytes(stderr)
            events = []
            for line in stdout.splitlines():
                try:
                    events.append(json.loads(line))
                except json.JSONDecodeError:
                    pass
            result = summarize(events)
            result.update(
                case=case_name,
                variant=variant,
                model=args.model,
                exit_code=code,
                stopped=stopped,
                seconds=round(time.monotonic() - started, 2),
                independent_check=None,
                human_claim_review="pending",
            )
            if case["check"] is not None:
                check_code, _, check_stderr, check_stop = await run_bounded(
                    [
                        str(args.binary),
                        "sandbox",
                        "-P",
                        ":workspace",
                        "-C",
                        str(directory),
                        "--",
                        sys.executable,
                        "-c",
                        case["check"],
                    ],
                    directory,
                    15,
                )
                result["independent_check"] = check_code == 0 and check_stop is None
                result["grader"] = {
                    "exit_code": check_code,
                    "stopped": check_stop,
                    "stderr": check_stderr[-4096:].decode(errors="replace"),
                }
            results.append(result)
            (args.output / "results.json").write_text(
                json.dumps(results, indent=2) + "\n"
            )
            print(
                json.dumps(
                    {
                        key: result[key]
                        for key in (
                            "case",
                            "variant",
                            "exit_code",
                            "seconds",
                            "independent_check",
                            "usage",
                            "stopped",
                        )
                    }
                ),
                flush=True,
            )
            if not result["turn_completed"]:
                print(
                    "Stopping evaluation after an incomplete turn; inspect the recorded error.",
                    flush=True,
                )
                raise SystemExit(1)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--catalog", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--model", default="Qwen/Qwen3.8-Flash-Next")
    parser.add_argument("--effort", default="xhigh")
    parser.add_argument("--timeout", type=float, default=180)
    parser.add_argument("--cases", nargs="+", choices=CASES, default=list(CASES))
    parser.add_argument(
        "--variants",
        nargs="+",
        choices=("baseline", "candidate"),
        default=["baseline", "candidate"],
    )
    args = parser.parse_args()
    if os.name != "posix":
        parser.error(
            "Live evaluation currently requires macOS or Linux process-group containment"
        )
    for key in ("binary", "catalog", "candidate", "output"):
        setattr(args, key, getattr(args, key).resolve())
    if args.output.exists() or not 0 < args.timeout <= 600:
        parser.error("Use a new output directory and timeout in (0, 600]")
    asyncio.run(evaluate(args))


if __name__ == "__main__":
    main()
