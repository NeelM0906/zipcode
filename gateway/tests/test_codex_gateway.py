import importlib.util
import sys
import types
import unittest
from copy import deepcopy
from pathlib import Path
from unittest.mock import patch


def _import_gateway_module():
    protocol_module = types.ModuleType("sglang.srt.entrypoints.openai.protocol")
    protocol_module.ResponsesRequest = type("ResponsesRequest", (), {})
    modules = {
        "sglang": types.ModuleType("sglang"),
        "sglang.srt": types.ModuleType("sglang.srt"),
        "sglang.srt.entrypoints": types.ModuleType("sglang.srt.entrypoints"),
        "sglang.srt.entrypoints.openai": types.ModuleType(
            "sglang.srt.entrypoints.openai"
        ),
        "sglang.srt.entrypoints.openai.protocol": protocol_module,
    }
    module_path = Path(__file__).parents[1] / "codex_gateway.py"
    spec = importlib.util.spec_from_file_location(
        "_zipcode_test_codex_gateway", module_path
    )
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load gateway module for tests")
    codex_gateway = importlib.util.module_from_spec(spec)
    with patch.dict(sys.modules, modules):
        spec.loader.exec_module(codex_gateway)

    return codex_gateway


codex_gateway = _import_gateway_module()


def _function_tool(name: str) -> dict[str, object]:
    return {
        "type": "function",
        "name": name,
        "description": f"Run {name}.",
        "parameters": {"type": "object", "properties": {}},
    }


def _guardian_format() -> dict[str, object]:
    return {
        "type": "json_schema",
        "name": "guardian_assessment",
        "strict": False,
        "schema": {
            "type": "object",
            "properties": {"outcome": {"enum": ["allow", "deny"]}},
            "required": ["outcome"],
            "additionalProperties": False,
        },
    }


class ResponsesCompatibilityTests(unittest.TestCase):
    def test_guardian_constrained_request_omits_all_tools(self) -> None:
        text = {"format": _guardian_format()}
        message = {
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "Review this action."}],
        }
        payload = {
            "client_metadata": {"x-openai-subagent": "guardian"},
            "input": [
                {"type": "additional_tools", "tools": [_function_tool("exec")]},
                message,
            ],
            "tools": [_function_tool("shell")],
            "text": deepcopy(text),
        }

        codex_gateway._prepare_responses_request(payload)

        self.assertNotIn("tools", payload)
        self.assertEqual(payload["input"], [message])
        self.assertEqual(payload["text"], text)

    def test_full_model_guardian_does_not_leave_tool_choice_without_tools(self) -> None:
        payload = {
            "client_metadata": {"x-openai-subagent": "guardian"},
            "input": [],
            "tools": [_function_tool("shell")],
            "tool_choice": "auto",
            "text": {"format": _guardian_format()},
        }

        codex_gateway._prepare_responses_request(payload)

        self.assertEqual(
            payload["text"],
            {
                "format": {
                    "type": "json_schema",
                    "name": "guardian_assessment",
                    "strict": False,
                    "schema": {
                        "type": "object",
                        "properties": {"outcome": {"enum": ["allow", "deny"]}},
                        "required": ["outcome"],
                        "additionalProperties": False,
                    },
                }
            },
        )
        self.assertNotIn("tools", payload)
        self.assertNotIn("tool_choice", payload)

    def test_ordinary_unconstrained_request_still_lifts_additional_tools(self) -> None:
        tool = _function_tool("exec")
        message = {
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "Run it."}],
        }
        payload = {
            "client_metadata": {"x-openai-subagent": "guardian"},
            "input": [{"type": "additional_tools", "tools": [tool]}, message],
        }

        codex_gateway._prepare_responses_request(payload)

        self.assertEqual(payload["input"], [message])
        self.assertEqual(payload["tools"], [tool])

    def test_non_guardian_constrained_request_keeps_tools(self) -> None:
        tool = _function_tool("exec")
        text = {"format": _guardian_format()}
        payload = {
            "client_metadata": {"x-openai-subagent": "explorer"},
            "input": [],
            "tools": [tool],
            "text": deepcopy(text),
        }

        codex_gateway._prepare_responses_request(payload)

        self.assertEqual(payload["tools"], [tool])
        self.assertEqual(payload["text"], text)

    def test_guardian_malformed_format_keeps_tools_without_crashing(self) -> None:
        tool = _function_tool("exec")
        payload = {
            "client_metadata": {"x-openai-subagent": "guardian"},
            "input": [],
            "tools": [tool],
            "text": {"format": {"type": {"invalid": True}}},
        }

        codex_gateway._prepare_responses_request(payload)

        self.assertEqual(payload["tools"], [tool])


if __name__ == "__main__":
    unittest.main()
