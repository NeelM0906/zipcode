"""Exercise the guard against real files and real child process outcomes."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import sqlite3

from guard_config import install

import runtime_guard as guard


class RuntimeGuardTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "repo"
        self.root.mkdir()
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        self.source = self.root / "example.py"
        self.source.write_text("value = 1\n")
        self.database = Path(self.temp.name) / "state.sqlite"
        self.event = dict(session_id="session", turn_id="turn", cwd=str(self.root))

    def hook(self, event, **extra):
        return guard.handle(
            self.event | {"hook_event_name": event} | extra, self.database
        )

    def command(self, code):
        return [
            sys.executable,
            str(Path(guard.__file__).resolve()),
            "check",
            "--",
            sys.executable,
            "-c",
            code,
        ]

    def check(self, code="pass", call="check-1"):
        argv = self.command(code)
        import shlex

        command = shlex.join(argv)
        output = self.hook(
            "PreToolUse",
            tool_name="Bash",
            tool_use_id=call,
            tool_input={"command": command},
        )
        if output:
            return output
        result = subprocess.run(argv, cwd=self.root, text=True, capture_output=True)
        self.hook(
            "PostToolUse",
            tool_name="Bash",
            tool_use_id=call,
            tool_input={"command": command},
            tool_response=result.stdout,
        )
        return result

    def test_read_only_turn_does_not_require_a_check(self):
        self.hook("UserPromptSubmit")
        self.assertEqual(self.hook("Stop"), {})

    def test_changed_source_requires_completed_check_and_later_edit_invalidates_it(
        self,
    ):
        self.hook("UserPromptSubmit")
        self.source.write_text("value = 2\n")
        self.assertEqual(self.hook("Stop")["decision"], "block")
        result = self.check()
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.hook("Stop", stop_hook_active=True), {})
        self.source.write_text("value = 3\n")
        self.assertFalse(self.hook("Stop", stop_hook_active=True)["continue"])

    def test_failed_check_is_not_evidence_and_fourth_identical_attempt_is_denied(self):
        self.hook("UserPromptSubmit")
        self.source.write_text("value = 2\n")
        for index in range(3):
            self.assertEqual(
                self.check("raise SystemExit(1)", str(index)).returncode, 1
            )
        denied = self.check("raise SystemExit(1)", "fourth")
        self.assertEqual(denied["hookSpecificOutput"]["permissionDecision"], "deny")
        self.source.write_text("value = 3\n")
        self.assertEqual(self.check("raise SystemExit(1)", "revised").returncode, 1)

    def test_check_that_changes_source_cannot_certify_its_own_changes(self):
        self.hook("UserPromptSubmit")
        self.check("from pathlib import Path; Path('example.py').write_text('changed')")
        self.assertEqual(self.hook("Stop")["decision"], "block")

    def test_unrelated_tool_cannot_supply_a_verification_receipt(self):
        self.hook("UserPromptSubmit")
        self.source.write_text("changed")
        fingerprint = guard.snapshot(self.root)
        self.hook(
            "PostToolUse",
            tool_name="Bash",
            tool_use_id="fake",
            tool_input={"command": "echo done"},
            tool_response=guard.MARKER
            + json.dumps(dict(before=fingerprint, after=fingerprint, exit_code=0)),
        )
        self.assertEqual(self.hook("Stop")["decision"], "block")

    def test_honest_blocker_can_finish_without_a_retry_loop(self):
        self.hook("UserPromptSubmit")
        self.source.write_text("changed")
        self.assertEqual(
            self.hook(
                "Stop",
                last_assistant_message="Verification: blocked — required service is unavailable.",
            ),
            {},
        )

    def test_ignored_build_artifacts_do_not_invalidate_evidence(self):
        (self.root / ".gitignore").write_text("target/\n")
        self.hook("UserPromptSubmit")
        self.source.write_text("changed")
        self.check()
        (self.root / "target").mkdir()
        (self.root / "target" / "artifact").write_text("build")
        self.assertEqual(self.hook("Stop"), {})

    def test_new_turn_cannot_reuse_old_turn_evidence(self):
        self.hook("UserPromptSubmit")
        self.check()
        self.event["turn_id"] = "next"
        self.hook("UserPromptSubmit")
        self.source.write_text("changed")
        self.assertEqual(self.hook("Stop")["decision"], "block")

    def test_snapshot_refuses_symlinks_outside_repository(self):
        outside = Path(self.temp.name) / "secret"
        outside.write_text("not read")
        (self.root / "link").symlink_to(outside)
        before = guard.snapshot(self.root)
        outside.write_text("different secret")
        self.assertEqual(guard.snapshot(self.root), before)

    def test_nested_workspace_detects_staged_and_sibling_changes(self):
        nested = self.root / "nested"
        nested.mkdir()
        source = nested / "app.py"
        source.write_text("first")
        subprocess.run(["git", "-C", str(self.root), "add", "."], check=True)
        before = guard.snapshot(nested)
        source.write_text("second")
        subprocess.run(["git", "-C", str(self.root), "add", "."], check=True)
        self.assertNotEqual(guard.snapshot(nested), before)
        before = guard.snapshot(nested)
        self.source.write_text("sibling change")
        self.assertNotEqual(guard.snapshot(nested), before)

    def test_missing_receipt_invalidates_success_and_bounds_retries(self):
        import shlex

        self.hook("UserPromptSubmit")
        self.source.write_text("changed")
        self.check()
        command = shlex.join(self.command("pass"))
        for index in range(3):
            self.assertEqual(
                self.hook(
                    "PreToolUse",
                    tool_name="Bash",
                    tool_use_id=str(index),
                    tool_input={"command": command},
                ),
                {},
            )
            self.hook(
                "PostToolUse",
                tool_name="Bash",
                tool_use_id=str(index),
                tool_input={"command": command},
                tool_response="command timed out",
            )
        self.assertEqual(self.hook("Stop")["decision"], "block")
        denied = self.hook(
            "PreToolUse",
            tool_name="Bash",
            tool_use_id="fourth",
            tool_input={"command": command},
        )
        self.assertEqual(denied["hookSpecificOutput"]["permissionDecision"], "deny")

    def test_missing_baseline_cannot_certify_current_source(self):
        with self.assertRaises(ValueError):
            self.hook("Stop")

    def test_busy_database_denies_pre_tool_use_within_hook_budget(self):
        self.hook("UserPromptSubmit")
        with sqlite3.connect(self.database) as connection:
            connection.execute("BEGIN EXCLUSIVE")
            result = subprocess.run(
                [
                    sys.executable,
                    str(Path(guard.__file__).resolve()),
                    "hook",
                    "--database",
                    str(self.database),
                ],
                input=json.dumps(self.event | {"hook_event_name": "PreToolUse"}),
                text=True,
                capture_output=True,
                timeout=6,
            )
            connection.rollback()
        self.assertEqual(result.returncode, 0)
        self.assertEqual(
            json.loads(result.stdout)["hookSpecificOutput"]["permissionDecision"],
            "deny",
        )

    def test_activation_refuses_existing_hooks_without_modifying_them(self):
        home = Path(self.temp.name) / "home"
        home.mkdir()
        (home / "config.toml").write_text("# existing configuration")
        (home / "hooks.json").write_text('{"existing": true}')
        with self.assertRaises(ValueError):
            install(home)
        self.assertEqual((home / "hooks.json").read_text(), '{"existing": true}')
        self.assertFalse((home / "runtime_guard.py").exists())


if __name__ == "__main__":
    unittest.main()
