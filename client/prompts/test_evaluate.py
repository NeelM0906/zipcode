import asyncio
import os
import signal
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from eval_cases import CASES
from evaluate import run_bounded, summarize


class EvaluationTests(unittest.TestCase):
    def test_missing_usage_is_unknown_not_free_and_claims_are_not_passes(self):
        result = summarize(
            [
                {
                    "type": "item.completed",
                    "item": {"type": "agent_message", "text": "All tests passed!"},
                }
            ]
        )
        self.assertIsNone(result["usage"])
        self.assertFalse(result["turn_completed"])
        self.assertEqual(result["final"], "All tests passed!")

    def test_counts_completed_commands_once_and_retains_reasoning_separately(self):
        command = {
            "id": "one",
            "type": "command_execution",
            "command": "python test.py",
            "exit_code": 1,
        }
        usage = {
            "input_tokens": 200,
            "output_tokens": 50,
            "reasoning_output_tokens": 30,
        }
        events = [
            {"type": "item.started", "item": command},
            {"type": "item.completed", "item": command},
            {"type": "item.completed", "item": {**command, "id": "two"}},
            {"type": "turn.completed", "usage": usage},
        ]
        result = summarize(events)
        self.assertEqual(result["usage"], usage)
        self.assertEqual(result["command_count"], 2)
        self.assertEqual(result["repeated_commands"], 1)
        self.assertTrue(result["turn_completed"])

    def test_install_grader_rejects_hardcoded_environment(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "bin").mkdir()
            (root / "settings.json").write_text('{"environment": "production"}')
            (root / "bin/demo.py").write_text('print("production")')
            result = subprocess.run(
                [sys.executable, "-c", CASES["install"]["check"]],
                cwd=root,
                capture_output=True,
            )
            self.assertNotEqual(result.returncode, 0)

    def test_partial_usage_does_not_turn_unknown_reasoning_into_zero(self):
        result = summarize(
            [
                {
                    "type": "turn.completed",
                    "usage": {"output_tokens": 5, "reasoning_output_tokens": 2},
                },
                {"type": "turn.completed", "usage": {"output_tokens": 3}},
            ]
        )
        self.assertEqual(
            result["usage"], {"output_tokens": 8, "reasoning_output_tokens": None}
        )

    def test_feature_grader_rejects_separate_hashability_buckets(self):
        broken = "def stable_unique(values):\n    seen = []\n    for value in values:\n        if not any(type(value) is type(old) and value == old for old in seen):\n            seen.append(value)\n    return seen\n"
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / "app.py").write_text(broken)
            result = subprocess.run(
                [sys.executable, "-c", CASES["feature"]["check"]],
                cwd=directory,
                capture_output=True,
            )
            self.assertNotEqual(result.returncode, 0)


@unittest.skipUnless(os.name == "posix", "POSIX process-group evaluation")
class ProcessTests(unittest.IsolatedAsyncioTestCase):
    async def test_completed_parent_does_not_leave_background_children(self):
        command = "import subprocess,sys; p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(20)'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL); print(p.pid)"
        with tempfile.TemporaryDirectory() as directory:
            result = await run_bounded(
                [sys.executable, "-c", command], Path(directory), 5
            )
            child = int(result[1].strip())
            alive = True
            try:
                for _ in range(20):
                    try:
                        os.kill(child, 0)
                    except ProcessLookupError:
                        alive = False
                        break
                    await asyncio.sleep(0.05)
                self.assertFalse(
                    alive, "owned background child survived parent completion"
                )
            finally:
                if alive:
                    try:
                        os.kill(child, signal.SIGKILL)
                    except ProcessLookupError:
                        pass

    async def test_excess_output_stops_and_drains_without_hanging(self):
        with tempfile.TemporaryDirectory() as directory:
            result = await asyncio.wait_for(
                run_bounded(
                    [
                        sys.executable,
                        "-c",
                        "import os\nwhile True: os.write(1, b'x'*65536)",
                    ],
                    Path(directory),
                    2,
                ),
                10,
            )
            self.assertEqual(result[3], "output_limit")
            self.assertLessEqual(len(result[1]) + len(result[2]), 8 * 1024 * 1024)

    async def test_timeout_is_not_success(self):
        with tempfile.TemporaryDirectory() as directory:
            result = await run_bounded(
                [sys.executable, "-c", "import time; time.sleep(30)"],
                Path(directory),
                0.1,
            )
            self.assertEqual(result[3], "timeout")
            self.assertNotEqual(result[0], 0)

    async def test_ambient_secrets_and_python_optimization_are_not_inherited(self):
        with (
            tempfile.TemporaryDirectory() as directory,
            patch.dict(
                os.environ, {"EVAL_TEST_SECRET": "sentinel", "PYTHONOPTIMIZE": "2"}
            ),
        ):
            result = await run_bounded(
                [
                    sys.executable,
                    "-c",
                    "import os; assert 'EVAL_TEST_SECRET' not in os.environ; assert __debug__",
                ],
                Path(directory),
                5,
            )
            self.assertEqual(result[0], 0)


if __name__ == "__main__":
    unittest.main()
