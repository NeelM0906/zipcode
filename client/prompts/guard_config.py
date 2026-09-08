"""Install the macOS/Linux hook adapter separately from experimental prompts."""

import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import sys


def install(home: Path) -> None:
    if os.name != "posix":
        raise ValueError("Runtime guard adapter currently supports macOS/Linux only")
    home = home.resolve(strict=True)
    if not (home / "config.toml").is_file():
        raise ValueError("Expected an existing ZIPCODE configuration directory")
    targets = [
        home / name
        for name in ("hooks.json", "runtime_guard.py", "verification.sqlite")
    ]
    if any(path.exists() or path.is_symlink() for path in targets):
        raise ValueError(
            "Guard files or hooks already exist; merge/review them explicitly"
        )
    shutil.copyfile(Path(__file__).with_name("runtime_guard.py"), targets[1])
    targets[1].chmod(0o600)
    command = shlex.join(
        [sys.executable, str(targets[1]), "hook", "--database", str(targets[2])]
    )
    hooks = {}
    for event in (
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "Stop",
        "SubagentStop",
    ):
        group = {"hooks": [{"type": "command", "command": command, "timeout": 15}]}
        if event not in ("Stop", "SubagentStop"):
            group["hooks"][0]["additionalContextLimit"] = 1024
        if event in ("PreToolUse", "PostToolUse"):
            group["matcher"] = "Bash"
        hooks[event] = [group]
    targets[0].write_text(
        json.dumps({"hooks": hooks}, indent=2) + "\n", encoding="utf-8"
    )
    targets[0].chmod(0o600)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--home", required=True, type=Path)
    install(parser.parse_args().home)
