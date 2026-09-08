#!/usr/bin/env python3
"""Bounded verification receipts and lifecycle hooks; not a security boundary."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import signal
import sqlite3
import stat
import subprocess
import sys
import tempfile
import time


MARKER = "ZIPCODE_VERIFICATION_RECEIPT:"
MAX_INPUT = 1024 * 1024
MAX_FILES = 10000
MAX_BYTES = 64 * 1024 * 1024
SCRIPT = str(Path(__file__).resolve())


def snapshot(cwd):
    """Hash the Git revision plus changed/non-ignored files; ignore build caches."""
    cwd = Path(cwd).resolve()
    root = subprocess.run(
        ["git", "-C", str(cwd), "rev-parse", "--show-toplevel"],
        capture_output=True,
        timeout=5,
    )
    if root.returncode:
        raise ValueError("verification requires a Git workspace")
    cwd = Path(os.fsdecode(root.stdout).rstrip("\r\n")).resolve()
    with tempfile.TemporaryFile() as output:
        for arguments in (
            [
                "ls-files",
                "-z",
                "--modified",
                "--deleted",
                "--others",
                "--exclude-standard",
            ],
            ["diff", "--cached", "--name-only", "-z", "--no-ext-diff", "--no-textconv"],
        ):
            result = subprocess.run(
                ["git", "-c", "core.fsmonitor=false", "-C", str(cwd), *arguments],
                stdout=output,
                stderr=subprocess.DEVNULL,
                timeout=5,
            )
            if result.returncode or output.tell() > MAX_INPUT:
                raise ValueError("verification requires a bounded Git workspace")
        output.seek(0)
        paths = sorted(set(output.read().split(b"\0")) - {b""})
    if len(paths) > MAX_FILES:
        raise ValueError("verification workspace exceeds file budget")
    digest = hashlib.sha256()
    revision = subprocess.run(
        ["git", "-C", str(cwd), "rev-parse", "--verify", "HEAD"],
        capture_output=True,
        timeout=5,
    )
    digest.update(revision.stdout if revision.returncode == 0 else b"unborn")
    total = 0
    for raw in paths:
        path = cwd / os.fsdecode(raw)
        digest.update(raw + b"\0")
        # Never follow a symlink, including a symlink in a parent directory.
        if not path.parent.resolve().is_relative_to(cwd):
            raise ValueError("verification path escapes workspace")
        try:
            metadata = path.lstat()
        except FileNotFoundError:
            digest.update(b"deleted\0")
            continue
        digest.update(str(stat.S_IMODE(metadata.st_mode)).encode() + b"\0")
        if path.is_symlink():
            digest.update(os.fsencode(os.readlink(path)))
        elif stat.S_ISREG(metadata.st_mode):
            total += metadata.st_size
            if total > MAX_BYTES:
                raise ValueError("verification workspace exceeds byte budget")
            with path.open("rb") as source:
                contents = source.read(MAX_BYTES - total + metadata.st_size + 1)
            after = path.stat()
            if len(contents) != metadata.st_size or any(
                getattr(after, field) != getattr(metadata, field)
                for field in (
                    "st_ino",
                    "st_mode",
                    "st_size",
                    "st_mtime_ns",
                    "st_ctime_ns",
                )
            ):
                raise ValueError("workspace changed during verification snapshot")
            digest.update(contents)
        else:
            raise ValueError(
                "verification cannot fingerprint a submodule or special file"
            )
        digest.update(b"\0")
    return digest.hexdigest()


def check_command(tool_input):
    """Accept only a direct receipt runner, not arbitrary output containing a marker."""
    command = tool_input.get("command", "")
    try:
        argv = shlex.split(command, posix=os.name != "nt")
    except (ValueError, TypeError):
        return None
    if len(argv) < 5 or argv[1:4] != [SCRIPT, "check", "--"]:
        return None
    if Path(argv[0]).resolve() != Path(sys.executable).resolve():
        return None
    return hashlib.sha256(command.encode()).hexdigest()


def transition(event, state):
    current = snapshot(event["cwd"])
    name = event["hook_event_name"]
    if "baseline" not in state:
        if name != "UserPromptSubmit":
            raise ValueError("verification baseline is missing; start a new turn")
        state["baseline"] = current
    state.setdefault("failures", {})
    state.setdefault("pending", {})
    command = check_command(event.get("tool_input", {}))
    call = event.get("tool_use_id", "")
    if name == "UserPromptSubmit":
        runner = shlex.join([sys.executable, SCRIPT, "check", "--"])
        return {
            "hookSpecificOutput": {
                "hookEventName": name,
                "additionalContext": f"Verification guard active for Git source changes. Run your smallest meaningful check through the normal shell tool as: {runner} <command> [args]. A successful receipt remains valid until source changes; do not repeat it unnecessarily. The check must exercise the requested result, not merely exit zero. If blocked, finish with 'Verification: blocked — <specific reason>'.",
            }
        }
    if name == "PreToolUse" and command and event.get("tool_name") == "Bash":
        key = command + current
        if state["failures"].get(key, 0) >= 3:
            return {
                "hookSpecificOutput": {
                    "hookEventName": name,
                    "permissionDecision": "deny",
                    "permissionDecisionReason": "Three identical checks failed against unchanged source. Revise the check or implementation, or report the blocker; do not retry unchanged.",
                }
            }
        if len(state["pending"]) >= 16:
            state["pending"].pop(next(iter(state["pending"])))
        state.pop("verified", None)
        state["pending"][call] = [key, current]
    elif name == "PostToolUse" and command and call in state["pending"]:
        key, before = state["pending"].pop(call)
        response = event.get("tool_response", "")
        marker = response.rsplit("\n" + MARKER, 1) if isinstance(response, str) else []
        try:
            receipt = json.loads(marker[1]) if len(marker) == 2 else {}
        except (ValueError, TypeError):
            receipt = {}
        if (
            isinstance(receipt, dict)
            and type(receipt.get("exit_code")) is int
            and receipt["exit_code"] == 0
            and receipt.get("before") == before
            and receipt.get("after") == current
            and before == current
        ):
            state["verified"] = current
            state["failures"].pop(key, None)
        else:
            state.pop("verified", None)
            failures = state["failures"]
            failures[key] = min(failures.get(key, 0) + 1, 3)
            while len(failures) > 16:
                failures.pop(next(iter(failures)))
    elif name in ("Stop", "SubagentStop"):
        if current in (state["baseline"], state.get("verified")):
            return {}
        final = event.get("last_assistant_message") or ""
        if re.search(r"(?im)^verification:\s*(blocked|not run|failed)\b.{12,}", final):
            return {}
        if event.get("stop_hook_active") or state.get("stop_requested"):
            return {
                "continue": False,
                "stopReason": "ZIPCODE verification is incomplete: source changed without a fresh successful check. The task must not be reported as verified.",
            }
        state["stop_requested"] = True
        runner = shlex.join([sys.executable, SCRIPT, "check", "--"])
        return {
            "decision": "block",
            "reason": f"Source changed without fresh verification. Run the smallest meaningful check through the normal shell tool: {runner} <command> [args]. This uses the existing sandbox and approvals. If verification is unavailable, finish with 'Verification: blocked — <specific reason>'. Do not repeat unchanged failed checks.",
        }
    return {}


def handle(event, database):
    """Persist only bounded hashes/counters, scoped by session, turn and workspace."""
    identity = [
        event["session_id"],
        event["turn_id"],
        str(Path(event["cwd"]).resolve()),
    ]
    key = hashlib.sha256(json.dumps(identity).encode()).hexdigest()
    with sqlite3.connect(database, timeout=2) as connection:
        connection.execute(
            "CREATE TABLE IF NOT EXISTS guards (id TEXT PRIMARY KEY, updated REAL, state TEXT)"
        )
        connection.execute("BEGIN IMMEDIATE")
        row = connection.execute(
            "SELECT state FROM guards WHERE id = ?", (key,)
        ).fetchone()
        state = json.loads(row[0]) if row else {}
        output = transition(event, state)
        connection.execute(
            "INSERT OR REPLACE INTO guards VALUES (?, ?, ?)",
            (key, time.time(), json.dumps(state)),
        )
        connection.execute(
            "DELETE FROM guards WHERE id NOT IN (SELECT id FROM guards ORDER BY updated DESC LIMIT 128)"
        )
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="mode", required=True)
    hook = commands.add_parser("hook")
    hook.add_argument("--database", type=Path, required=True)
    check = commands.add_parser("check")
    check.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if os.name != "posix":
        parser.error("This hook adapter supports macOS/Linux POSIX shells only")
    if args.mode == "check":
        command = args.command[1:] if args.command[:1] == ["--"] else args.command
        if not command:
            parser.error("check needs a command after --")
        before = snapshot(Path.cwd())
        try:
            code = subprocess.run(command, check=False).returncode
        except OSError:
            code = 127
        after = snapshot(Path.cwd())
        print(
            "\n"
            + MARKER
            + json.dumps(dict(before=before, after=after, exit_code=code)),
            flush=True,
        )
        return code if 0 <= code <= 255 else 1
    event = {}

    def deadline(_signum, _frame):
        raise TimeoutError("verification hook deadline exceeded")

    signal.signal(signal.SIGALRM, deadline)
    signal.setitimer(signal.ITIMER_REAL, 10)
    try:
        raw = sys.stdin.buffer.read(MAX_INPUT + 1)
        if len(raw) > MAX_INPUT:
            raise ValueError("hook input exceeds byte budget")
        event = json.loads(raw)
        output = handle(event, args.database)
    except (
        OSError,
        ValueError,
        KeyError,
        TypeError,
        sqlite3.Error,
        subprocess.TimeoutExpired,
    ) as error:
        reason = f"ZIPCODE verification guard unavailable ({type(error).__name__}); no verification result was recorded."
        output = (
            {
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": reason,
                }
            }
            if isinstance(event, dict) and event.get("hook_event_name") == "PreToolUse"
            else {"continue": False, "stopReason": reason}
        )
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
    print(json.dumps(output))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
