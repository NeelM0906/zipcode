"""Opt-in installed-CLI integration with a local deterministic inference fixture."""

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import threading
import unittest
import selectors

from guard_config import install


@unittest.skipUnless(
    os.environ.get("ZIPCODE_TEST_CORE"),
    "set ZIPCODE_TEST_CORE for installed-CLI integration",
)
class GuardCliTests(unittest.TestCase):
    def test_real_cli_blocks_unverified_completion_then_accepts_fresh_receipt(self):
        with tempfile.TemporaryDirectory(prefix="zipcode-guard-cli-") as temporary:
            root = Path(temporary).resolve()
            home, repo = root / "home", root / "repo"
            home.mkdir()
            repo.mkdir()
            subprocess.run(["git", "init", "-q", str(repo)], check=True)
            (repo / "app.py").write_text("value = 1\n")
            requests = []
            runner = shlex.join(
                [
                    sys.executable,
                    str(home / "runtime_guard.py"),
                    "check",
                    "--",
                    sys.executable,
                    "-c",
                    "from pathlib import Path; assert Path('app.py').read_text() == 'value = 2\\n'",
                ]
            )

            class Handler(BaseHTTPRequestHandler):
                def log_message(self, *_):
                    pass

                def do_POST(self):
                    body = json.loads(
                        self.rfile.read(int(self.headers["Content-Length"]))
                    )
                    requests.append(body)
                    index = len(requests)
                    command = {1: "printf 'value = 2\\n' > app.py", 3: runner}.get(
                        index
                    )
                    item = (
                        dict(
                            type="function_call",
                            call_id=f"call-{index}",
                            name="exec_command",
                            arguments=json.dumps(
                                {"cmd": command, "yield_time_ms": 1000}
                            ),
                        )
                        if command
                        else dict(
                            type="message",
                            role="assistant",
                            id=f"msg-{index}",
                            content=[dict(type="output_text", text="Done, verified.")],
                        )
                    )
                    events = [
                        dict(type="response.created", response={"id": str(index)}),
                        dict(type="response.output_item.done", item=item),
                        dict(type="response.completed", response={"id": str(index)}),
                    ]
                    data = "".join(
                        "data: " + json.dumps(event) + "\n\n" for event in events
                    ).encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    self.wfile.write(data)

            server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            self.addCleanup(server.server_close)
            self.addCleanup(server.shutdown)
            (home / "config.toml").write_text(
                'model = "Qwen/Qwen3.8-Flash-Next"\nmodel_provider = "fixture"\ncheck_for_update_on_startup = false\n'
                f"model_catalog_json = {json.dumps(str(Path(__file__).resolve().parents[1] / 'models.json'))}\n"
                f"model_instructions_file = {json.dumps(str(Path(__file__).resolve().with_name('shared.md')))}\n"
                'approval_policy = "never"\nsandbox_mode = "workspace-write"\nweb_search = "disabled"\n'
                '[features]\nplugins = false\n'
                '[model_providers.fixture]\nname = "Local test fixture"\nwire_api = "responses"\n'
                f'base_url = "http://127.0.0.1:{server.server_port}/v1"\nrequest_max_retries = 0\nstream_max_retries = 0\n'
                f'[projects.{json.dumps(str(repo))}]\ntrust_level = "trusted"\n'
            )
            install(home)
            environment = {
                key: value
                for key, value in os.environ.items()
                if key in ("HOME", "PATH", "TMPDIR", "USER", "LANG")
            }
            environment.update(
                CODEX_HOME=str(home),
                ZIPCODE_HOME=str(home),
                ZIPCODE_PRIVATE_MODE="1",
                ZIPCODE_DISABLE_TRACE_UPLOAD="1",
            )
            # Approve only this test's hooks using the engine's own normalized hashes.
            process = subprocess.Popen(
                [os.environ["ZIPCODE_TEST_CORE"], "app-server"],
                env=environment,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                text=True,
                bufsize=1,
            )
            try:

                def request(identifier, method, params):
                    process.stdin.write(
                        json.dumps(dict(id=identifier, method=method, params=params))
                        + "\n"
                    )
                    process.stdin.flush()
                    selector = selectors.DefaultSelector()
                    try:
                        selector.register(process.stdout, selectors.EVENT_READ)
                        while selector.select(10):
                            line = process.stdout.readline()
                            if not line:
                                break
                            message = json.loads(line)
                            if message.get("id") == identifier:
                                if "error" in message:
                                    raise AssertionError(message["error"])
                                return message["result"]
                    finally:
                        selector.close()
                    raise AssertionError("app-server response timed out")

                request(
                    1,
                    "initialize",
                    {
                        "clientInfo": {"name": "zipcode-guard-test", "version": "1"},
                        "capabilities": {"experimentalApi": True},
                    },
                )
                listed = request(2, "hooks/list", {"cwds": [str(repo)]})
            finally:
                process.terminate()
                process.communicate(timeout=5)
            approvals = []
            for group in listed["data"]:
                for hook in group["hooks"]:
                    self.assertEqual(Path(hook["sourcePath"]), home / "hooks.json")
                    approvals.append(
                        f"\n[hooks.state.{json.dumps(hook['key'])}]\ntrusted_hash={json.dumps(hook['currentHash'])}\n"
                    )
            self.assertEqual(len(approvals), 5)
            with (home / "config.toml").open("a") as config:
                config.write("".join(approvals))
            result = subprocess.run(
                [
                    os.environ["ZIPCODE_TEST_CORE"],
                    "exec",
                    "--json",
                    "--ephemeral",
                    "--skip-git-repo-check",
                    "-C",
                    str(repo),
                    "Change value to 2 and verify.",
                ],
                env=environment,
                capture_output=True,
                text=True,
                timeout=45,
            )
            self.assertEqual(
                result.returncode, 0, result.stderr[-4000:] + result.stdout[-4000:]
            )
            import sqlite3

            database = home / "verification.sqlite"
            state = (
                sqlite3.connect(database).execute("select state from guards").fetchall()
                if database.exists()
                else "no hook database"
            )
            self.assertEqual(
                len(requests),
                4,
                result.stdout[-4000:] + result.stderr[-2000:] + str(state),
            )
            self.assertIn(
                "Source changed without fresh verification", json.dumps(requests[2])
            )
            self.assertIn("ZIPCODE_VERIFICATION_RECEIPT", json.dumps(requests[3]))
            self.assertEqual((repo / "app.py").read_text(), "value = 2\n")
            shared = Path(__file__).resolve().with_name("shared.md").read_text().strip()
            developer = "".join(
                part.get("text", "")
                for item in requests[0].get("input", [])
                if item.get("role") == "developer"
                for part in item.get("content", [])
            )
            self.assertEqual(developer.count(shared), 1)
            if os.environ.get("ZIPCODE_REPORT_PROMPT_SIZE"):
                first = requests[0]
                sizes = {
                    "base_characters": len(first.get("instructions", "")),
                    "tool_schema_json_characters": len(
                        json.dumps(first.get("tools", []))
                    ),
                }
                sizes["shared_base_text_characters"] = len(shared)
                sizes["other_developer_text_characters"] = len(developer) - len(shared)
                for role in ("developer", "user"):
                    sizes[f"{role}_input_json_characters"] = len(
                        json.dumps(
                            [
                                item
                                for item in first.get("input", [])
                                if item.get("role") == role
                            ]
                        )
                    )
                    sizes[f"{role}_text_characters"] = sum(
                        len(part.get("text", ""))
                        for item in first.get("input", [])
                        if item.get("role") == role
                        for part in item.get("content", [])
                    )
                print(
                    json.dumps(
                        {
                            "synthetic_request_components": sizes,
                            "token_counts": "not measured; character counts are not tokens",
                        }
                    )
                )


if __name__ == "__main__":
    unittest.main()
