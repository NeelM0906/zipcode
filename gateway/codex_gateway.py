"""Codex Responses compatibility gateway for the local Qwen3.8 service.

Codex can group callable tools under Responses API ``namespace`` entries.
SGLang's Responses frontend currently accepts the individual function tools but
not the namespace wrapper.  This proxy flattens namespaced tools on the request
path and restores the namespace field on function-call response items.

The proxy deliberately logs tool metadata only; prompts, arguments, tool output,
and authorization headers are never logged.
"""

from __future__ import annotations

import hashlib
import json
import logging
import os
import re
import threading
import time
import uuid
from collections import defaultdict
from copy import deepcopy
from collections.abc import AsyncIterator
from typing import Any

import httpx
from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse, Response, StreamingResponse
from pydantic import ValidationError
from sglang.srt.entrypoints.openai.protocol import ResponsesRequest


UPSTREAM = os.environ.get("QWEN_CODEX_UPSTREAM", "http://127.0.0.1:8003").rstrip("/")
MODEL = os.environ.get("QWEN_CODEX_MODEL", "Qwen/Qwen3.8-27B-FP8")
MODEL_CACHE = os.environ.get("QWEN_CODEX_MODEL_CACHE", "/gateway/models_cache.json")
MODEL_DISPLAY_NAME = os.environ.get(
    "QWEN_CODEX_DISPLAY_NAME", "Qwen3.8-27B FP8 (local)"
)
MODEL_DESCRIPTION = os.environ.get(
    "QWEN_CODEX_DESCRIPTION", "Local Qwen3.8-27B FP8 coding agent with 1M context."
)
MODEL_CONTEXT_WINDOW = int(os.environ.get("QWEN_CODEX_CONTEXT_WINDOW", "1000000"))
MODEL_EFFECTIVE_CONTEXT_PERCENT = int(
    os.environ.get("QWEN_CODEX_EFFECTIVE_CONTEXT_PERCENT", "85")
)
MODEL_IDENTITY = os.environ.get(
    "QWEN_CODEX_IDENTITY", "the local Qwen3.8-27B model"
)
NAME_LIMIT = 64
FLAT_SEPARATOR = "__"
SGLANG_TOOL_TYPES = {"function", "web_search_preview", "code_interpreter", "mcp"}
QWEN_REASONING_TEMPLATE_EFFORTS = {
    "low": "low",
    "medium": "medium",
    "high": "xhigh",
    "xhigh": "xhigh",
    "max": "xhigh",
    "ultra": "xhigh",
}

logging.basicConfig(
    level=os.environ.get("QWEN_CODEX_LOG_LEVEL", "INFO").upper(),
    format="%(asctime)s %(levelname)s %(message)s",
)
LOG = logging.getLogger("qwen-codex-gateway")

app = FastAPI(title="Qwen3.8 Codex Responses Gateway", version="1.0.0")
client: httpx.AsyncClient | None = None

METRIC_LOCK = threading.Lock()
REQUESTS: dict[tuple[str, int], int] = defaultdict(int)
INFLIGHT = 0
DURATION_BUCKETS = (0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 300.0, 900.0)
DURATION_COUNTS: dict[tuple[str, float], int] = defaultdict(int)
DURATION_SUMS: dict[str, float] = defaultdict(float)
DURATION_TOTALS: dict[str, int] = defaultdict(int)
TRANSFORMATIONS: dict[str, int] = defaultdict(int)


def _clean_component(value: str) -> str:
    cleaned = re.sub(r"[^A-Za-z0-9_-]", "_", value)
    return cleaned or "tool"


def _flat_name(namespace: str, name: str) -> str:
    """Return a deterministic SGLang-safe function name no longer than 64 chars."""
    raw = f"{_clean_component(namespace)}{FLAT_SEPARATOR}{_clean_component(name)}"
    if len(raw) <= NAME_LIMIT:
        return raw
    digest = hashlib.sha256(f"{namespace}\0{name}".encode()).hexdigest()[:12]
    return f"{raw[: NAME_LIMIT - len(digest) - 1]}_{digest}"


def _metric_endpoint(path: str) -> str:
    normalized = "/" + path.strip("/")
    if normalized in {"/v1/responses", "/v1/chat/completions", "/v1/completions"}:
        return normalized
    return "/other"


def _metric_start() -> None:
    global INFLIGHT
    with METRIC_LOCK:
        INFLIGHT += 1


def _metric_complete(endpoint: str, status: int, started: float) -> None:
    global INFLIGHT
    elapsed = time.monotonic() - started
    with METRIC_LOCK:
        INFLIGHT = max(0, INFLIGHT - 1)
        REQUESTS[(endpoint, status)] += 1
        DURATION_SUMS[endpoint] += elapsed
        DURATION_TOTALS[endpoint] += 1
        for bucket in DURATION_BUCKETS:
            if elapsed <= bucket:
                DURATION_COUNTS[(endpoint, bucket)] += 1


def _metric_transform(kind: str, count: int = 1) -> None:
    with METRIC_LOCK:
        TRANSFORMATIONS[kind] += count


def _prometheus_escape(value: str) -> str:
    return value.replace("\\", "\\\\").replace("\n", "\\n").replace('"', '\\"')


def _prometheus_metrics() -> str:
    lines = [
        "# HELP qwen_codex_gateway_info Static gateway identity.",
        "# TYPE qwen_codex_gateway_info gauge",
        f'qwen_codex_gateway_info{{model="{_prometheus_escape(MODEL)}"}} 1',
        "# HELP qwen_codex_gateway_inflight_requests Requests currently being proxied.",
        "# TYPE qwen_codex_gateway_inflight_requests gauge",
    ]
    with METRIC_LOCK:
        lines.append(f"qwen_codex_gateway_inflight_requests {INFLIGHT}")
        lines.extend(
            [
                "# HELP qwen_codex_gateway_requests_total Completed proxy requests.",
                "# TYPE qwen_codex_gateway_requests_total counter",
            ]
        )
        for (endpoint, status), count in sorted(REQUESTS.items()):
            lines.append(
                f'qwen_codex_gateway_requests_total{{endpoint="{endpoint}",status="{status}"}} {count}'
            )
        lines.extend(
            [
                "# HELP qwen_codex_gateway_request_duration_seconds End-to-end gateway request duration.",
                "# TYPE qwen_codex_gateway_request_duration_seconds histogram",
            ]
        )
        for endpoint in sorted(DURATION_TOTALS):
            for bucket in DURATION_BUCKETS:
                lines.append(
                    "qwen_codex_gateway_request_duration_seconds_bucket"
                    f'{{endpoint="{endpoint}",le="{bucket:g}"}} '
                    f"{DURATION_COUNTS[(endpoint, bucket)]}"
                )
            total = DURATION_TOTALS[endpoint]
            lines.append(
                "qwen_codex_gateway_request_duration_seconds_bucket"
                f'{{endpoint="{endpoint}",le="+Inf"}} {total}'
            )
            lines.append(
                f'qwen_codex_gateway_request_duration_seconds_sum{{endpoint="{endpoint}"}} '
                f"{DURATION_SUMS[endpoint]:.9f}"
            )
            lines.append(
                f'qwen_codex_gateway_request_duration_seconds_count{{endpoint="{endpoint}"}} {total}'
            )
        lines.extend(
            [
                "# HELP qwen_codex_gateway_transformations_total Codex compatibility transformations applied.",
                "# TYPE qwen_codex_gateway_transformations_total counter",
            ]
        )
        for kind, count in sorted(TRANSFORMATIONS.items()):
            lines.append(
                f'qwen_codex_gateway_transformations_total{{kind="{_prometheus_escape(kind)}"}} {count}'
            )
    return "\n".join(lines) + "\n"


def _flatten_tools(payload: dict[str, Any]) -> dict[str, tuple[str, str, str]]:
    mapping: dict[str, tuple[str, str, str]] = {}
    flattened: list[dict[str, Any]] = []
    dropped: dict[str, int] = {}

    for tool in payload.get("tools") or []:
        if not isinstance(tool, dict):
            flattened.append(tool)
            continue
        if tool.get("type") != "namespace":
            tool_type = str(tool.get("type") or "unknown")
            if tool_type in SGLANG_TOOL_TYPES:
                flattened.append(tool)
            else:
                dropped[tool_type] = dropped.get(tool_type, 0) + 1
            continue

        namespace = str(tool.get("name") or "namespace")
        namespace_description = str(tool.get("description") or "").strip()
        for member in tool.get("tools") or []:
            if not isinstance(member, dict):
                continue
            member_type = str(member.get("type") or "function")
            original_name = str(member.get("name") or "tool")
            encoded_name = _flat_name(namespace, original_name)
            if encoded_name in mapping and mapping[encoded_name][:2] != (
                namespace,
                original_name,
            ):
                suffix = hashlib.sha256(
                    f"{namespace}\0{original_name}".encode()
                ).hexdigest()[:16]
                encoded_name = f"gw_{suffix}"

            mapping[encoded_name] = (namespace, original_name, member_type)
            converted = dict(member)
            converted["type"] = "function"
            converted["name"] = encoded_name
            description = str(converted.get("description") or "").strip()
            context = f"Tool {original_name} in the {namespace} namespace."
            if namespace_description:
                context += f" Namespace: {namespace_description}"
            converted["description"] = f"{context} {description}".strip()

            # Custom/freeform namespace members are uncommon in Codex today.
            # Represent them as a single-string function so SGLang can still
            # expose them. Function members retain their original JSON schema.
            if member_type != "function":
                converted["parameters"] = {
                    "type": "object",
                    "properties": {"input": {"type": "string"}},
                    "required": ["input"],
                    "additionalProperties": False,
                }
                converted["strict"] = True
            flattened.append(converted)

    payload["tools"] = flattened
    if dropped:
        LOG.info("dropped unsupported Codex tool types=%s", dropped)
        _metric_transform("unsupported_tool_dropped", sum(dropped.values()))
    if mapping:
        _metric_transform("namespace_tool_flattened", len(mapping))
    return mapping


def _lift_additional_tools(payload: dict[str, Any]) -> int:
    """Move Responses-Lite additional_tools items to the standard tools field."""
    value = payload.get("input")
    if not isinstance(value, list):
        return 0
    retained: list[Any] = []
    lifted: list[dict[str, Any]] = []
    for item in value:
        if isinstance(item, dict) and item.get("type") == "additional_tools":
            lifted.extend(tool for tool in item.get("tools") or [] if isinstance(tool, dict))
        else:
            retained.append(item)
    if lifted:
        payload["input"] = retained
        payload["tools"] = list(payload.get("tools") or []) + lifted
        _metric_transform("additional_tool_lifted", len(lifted))
    return len(lifted)


def _omit_guardian_tools_for_constrained_output(payload: dict[str, Any]) -> int:
    """Omit tools when a Guardian review requires constrained output."""
    client_metadata = payload.get("client_metadata")
    if (
        not isinstance(client_metadata, dict)
        or client_metadata.get("x-openai-subagent") != "guardian"
    ):
        return 0

    text = payload.get("text")
    text_format = text.get("format") if isinstance(text, dict) else None
    if not isinstance(text_format, dict) or text_format.get("type") != "json_schema":
        return 0

    tools = payload.get("tools")
    if not isinstance(tools, list) or not tools:
        return 0

    omitted = len(tools)
    payload.pop("tools")
    _metric_transform("guardian_constrained_tool_omitted", omitted)
    return omitted


def _codex_model_catalog() -> dict[str, Any]:
    """Adapt Codex's cached model schema while preserving this model's identity."""
    with open(MODEL_CACHE, "r", encoding="utf-8") as handle:
        cache = json.load(handle)
    template = deepcopy(cache["models"][0])
    template.update(
        {
            "slug": MODEL,
            "display_name": MODEL_DISPLAY_NAME,
            "description": MODEL_DESCRIPTION,
            "default_reasoning_level": "xhigh",
            "supported_in_api": True,
            "priority": 1,
            "additional_speed_tiers": [],
            "service_tiers": [],
            "context_window": MODEL_CONTEXT_WINDOW,
            "max_context_window": MODEL_CONTEXT_WINDOW,
            "effective_context_window_percent": MODEL_EFFECTIVE_CONTEXT_PERCENT,
            "supports_search_tool": False,
            "experimental_supported_tools": [],
        }
    )
    for key in ("base_instructions",):
        value = template.get(key)
        if isinstance(value, str):
            template[key] = value.replace(
                "You are Codex, an agent based on GPT-5.",
                f"You are Codex, powered by {MODEL_IDENTITY}.",
                1,
            )
    messages = template.get("model_messages")
    if isinstance(messages, dict) and isinstance(messages.get("instructions_template"), str):
        messages["instructions_template"] = messages["instructions_template"].replace(
            "You are Codex, an agent based on GPT-5.",
            f"You are Codex, powered by {MODEL_IDENTITY}.",
            1,
        )
    return {"models": [template]}


def _flatten_input_calls(value: Any, mapping: dict[str, tuple[str, str, str]]) -> Any:
    reverse = {(namespace, name): encoded for encoded, (namespace, name, _) in mapping.items()}

    def visit(node: Any) -> None:
        if isinstance(node, dict):
            if node.get("type") == "message" and node.get("role") == "assistant":
                # Codex's stateless replay omits this field; SGLang's output
                # message variant requires it when content uses output_text.
                node.setdefault("status", "completed")
            if node.get("type") in {"function_call", "custom_tool_call"}:
                namespace = node.get("namespace")
                name = node.get("name")
                encoded = reverse.get((namespace, name))
                if encoded:
                    node["name"] = encoded
                    node.pop("namespace", None)
                    if node.get("type") == "custom_tool_call":
                        node["type"] = "function_call"
                        if "input" in node and "arguments" not in node:
                            node["arguments"] = json.dumps({"input": node.pop("input")})
            elif node.get("type") in {"custom_tool_call_output", "function_call_output"}:
                node["type"] = "function_call_output"
                output = node.get("output")
                if isinstance(output, list) and all(
                    isinstance(part, dict) and part.get("type") == "input_text"
                    for part in output
                ):
                    node["output"] = "".join(str(part.get("text") or "") for part in output)
            for child in node.values():
                visit(child)
        elif isinstance(node, list):
            for child in node:
                visit(child)

    visit(value)
    return value


def _restore_output_calls(value: Any, mapping: dict[str, tuple[str, str, str]]) -> Any:
    def visit(node: Any) -> None:
        if isinstance(node, dict):
            node_type = node.get("type")
            if node_type in {"function_call", "custom_tool_call"}:
                encoded = node.get("name")
                mapped = mapping.get(encoded)
                if mapped:
                    namespace, original_name, member_type = mapped
                    node["name"] = original_name
                    node["namespace"] = namespace
                    if member_type != "function":
                        node["type"] = "custom_tool_call"
                        arguments = node.pop("arguments", "{}")
                        try:
                            parsed = json.loads(arguments)
                            node["input"] = str(parsed.get("input", ""))
                        except (json.JSONDecodeError, AttributeError):
                            node["input"] = arguments
            elif node_type in {
                "response.function_call_arguments.done",
                "response.custom_tool_call_input.done",
            }:
                encoded = node.get("name")
                mapped = mapping.get(encoded)
                if mapped:
                    namespace, original_name, _ = mapped
                    node["name"] = original_name
                    node["namespace"] = namespace
            for child in node.values():
                visit(child)
        elif isinstance(node, list):
            for child in node:
                visit(child)

    visit(value)
    return value


def _prepare_responses_request(payload: dict[str, Any]) -> dict[str, tuple[str, str, str]]:
    lifted = _lift_additional_tools(payload)
    if lifted:
        LOG.info("lifted Responses-Lite additional tools=%d", lifted)
    mapping = _flatten_tools(payload)
    _flatten_input_calls(payload.get("input"), mapping)
    omitted = _omit_guardian_tools_for_constrained_output(payload)
    if omitted:
        # SGLang rejects tools combined with constrained decoding. Keep the
        # Guardian schema so its caller still requires a valid, fail-closed
        # approval assessment.
        LOG.info("omitted Guardian tools for constrained output=%d", omitted)
    if payload.get("stream") is None:
        payload["stream"] = False
    reasoning = payload.get("reasoning")
    if isinstance(reasoning, dict):
        requested_effort = str(reasoning.get("effort", "")).lower()
        template_effort = QWEN_REASONING_TEMPLATE_EFFORTS.get(requested_effort)
        if template_effort:
            template_kwargs = payload.get("chat_template_kwargs")
            template_kwargs = template_kwargs if isinstance(template_kwargs, dict) else {}
            template_kwargs["reasoning_effort"] = template_effort
            payload["chat_template_kwargs"] = template_kwargs
            # Qwen natively supports low, medium, and xhigh. ZIPCODE maps its
            # higher UI tiers to xhigh and uses medium on the wire because the
            # Rust Responses router still emits the unsupported `high` value.
            wire_effort = "medium" if template_effort == "xhigh" else template_effort
            reasoning["effort"] = wire_effort
            if requested_effort != wire_effort:
                _metric_transform("reasoning_effort_normalized")
                LOG.info(
                    "normalized router reasoning tier requested=%s template=%s wire=%s",
                    requested_effort,
                    template_effort,
                    wire_effort,
                )
    # SGLang does not currently use these OpenAI routing/cache hints. Keeping
    # them would cause strict-schema failures on some pinned builds.
    payload.pop("prompt_cache_options", None)
    payload.pop("safety_identifier", None)
    return mapping


def _input_shape(value: Any) -> Any:
    """Describe request structure without retaining prompt or tool-output text."""
    if isinstance(value, list):
        return [_input_shape(item) for item in value]
    if isinstance(value, dict):
        result: dict[str, Any] = {}
        for key, child in value.items():
            if key in {"text", "arguments", "encrypted_content"}:
                result[key] = f"<{type(child).__name__}>"
            elif key == "output" and isinstance(child, (dict, list)):
                result[key] = _input_shape(child)
            elif key == "output":
                result[key] = f"<{type(child).__name__}>"
            elif key in {"type", "role", "name", "namespace", "status"}:
                result[key] = child
            elif isinstance(child, (dict, list)):
                result[key] = _input_shape(child)
            else:
                result[key] = f"<{type(child).__name__}>"
        return result
    return f"<{type(value).__name__}>"


def _forward_headers(request: Request) -> dict[str, str]:
    excluded = {"host", "content-length", "connection", "transfer-encoding"}
    return {key: value for key, value in request.headers.items() if key.lower() not in excluded}


def _response_headers(response: httpx.Response) -> dict[str, str]:
    excluded = {
        "content-length",
        "content-encoding",
        "transfer-encoding",
        "connection",
        "keep-alive",
    }
    return {key: value for key, value in response.headers.items() if key.lower() not in excluded}


async def _sse_body(
    upstream_response: httpx.Response,
    mapping: dict[str, tuple[str, str, str]],
    endpoint: str,
    started: float,
) -> AsyncIterator[bytes]:
    try:
        async for line in upstream_response.aiter_lines():
            if line.startswith("data:"):
                data = line[5:].lstrip()
                if data and data != "[DONE]":
                    try:
                        event = json.loads(data)
                        _restore_output_calls(event, mapping)
                        line = "data: " + json.dumps(event, separators=(",", ":"))
                    except json.JSONDecodeError:
                        LOG.warning("upstream emitted a non-JSON SSE data event")
            yield (line + "\n").encode()
    finally:
        await upstream_response.aclose()
        _metric_complete(endpoint, upstream_response.status_code, started)


@app.on_event("startup")
async def startup() -> None:
    global client
    client = httpx.AsyncClient(timeout=None, limits=httpx.Limits(max_connections=512))
    LOG.info("gateway ready upstream=%s model=%s", UPSTREAM, MODEL)


@app.on_event("shutdown")
async def shutdown() -> None:
    if client is not None:
        await client.aclose()


@app.get("/health")
@app.get("/v1/health")
async def health() -> JSONResponse:
    assert client is not None
    started = time.monotonic()
    try:
        response = await client.get(f"{UPSTREAM}/health", timeout=5)
        healthy = response.is_success
    except httpx.HTTPError:
        healthy = False
    status = 200 if healthy else 503
    return JSONResponse(
        {
            "status": "ok" if healthy else "upstream_unavailable",
            "model": MODEL,
            "upstream": UPSTREAM,
            "latency_ms": round((time.monotonic() - started) * 1000, 2),
        },
        status_code=status,
    )


@app.get("/v1/models")
async def models(request: Request) -> Response:
    # Codex adds client_version and expects its model-catalog envelope. Other
    # OpenAI-compatible clients should continue to receive SGLang's data list.
    if request.query_params.get("client_version"):
        try:
            return JSONResponse(_codex_model_catalog())
        except (OSError, KeyError, IndexError, json.JSONDecodeError) as exc:
            LOG.error("could not construct Codex model catalog: %s", type(exc).__name__)
            return JSONResponse({"models": []}, status_code=500)
    assert client is not None
    response = await client.get(f"{UPSTREAM}/v1/models", headers=_forward_headers(request))
    return Response(
        content=response.content,
        status_code=response.status_code,
        headers=_response_headers(response),
        media_type=None,
    )


@app.get("/metrics")
async def metrics() -> Response:
    return Response(
        content=_prometheus_metrics(),
        media_type="text/plain; version=0.0.4; charset=utf-8",
    )


@app.api_route("/{path:path}", methods=["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"])
async def proxy(path: str, request: Request) -> Response:
    assert client is not None
    started = time.monotonic()
    endpoint = _metric_endpoint(path)
    _metric_start()
    request_id = request.headers.get("x-request-id") or f"qcg_{uuid.uuid4().hex}"
    body = await request.body()
    mapping: dict[str, tuple[str, str, str]] = {}

    if request.method == "POST" and path.rstrip("/") == "v1/responses":
        try:
            payload = json.loads(body or b"{}")
        except json.JSONDecodeError:
            _metric_complete(endpoint, 400, started)
            return JSONResponse({"error": {"message": "invalid JSON request body"}}, status_code=400)
        mapping = _prepare_responses_request(payload)
        body = json.dumps(payload, separators=(",", ":")).encode()
        LOG.info(
            "request id=%s endpoint=/v1/responses stream=%s tools=%d namespaced=%d namespaces=%s",
            request_id,
            payload.get("stream"),
            len(payload.get("tools") or []),
            len(mapping),
            sorted({namespace for namespace, _, _ in mapping.values()}),
        )
        LOG.info("request id=%s input_shape=%s", request_id, _input_shape(payload.get("input")))
        try:
            ResponsesRequest.model_validate(payload)
        except ValidationError as exc:
            LOG.error(
                "request id=%s local_schema_errors=%s",
                request_id,
                exc.errors(include_url=False, include_input=False),
            )

    url = f"{UPSTREAM}/{path}"
    if request.url.query:
        url += f"?{request.url.query}"
    upstream_request = client.build_request(
        request.method,
        url,
        headers=_forward_headers(request),
        content=body,
    )
    try:
        upstream_response = await client.send(upstream_request, stream=True)
    except httpx.HTTPError as exc:
        LOG.error("upstream failure id=%s type=%s", request_id, type(exc).__name__)
        _metric_complete(endpoint, 502, started)
        return JSONResponse(
            {"error": {"message": "Qwen inference upstream unavailable", "request_id": request_id}},
            status_code=502,
        )

    headers = _response_headers(upstream_response)
    headers["x-request-id"] = request_id
    content_type = upstream_response.headers.get("content-type", "")
    if "text/event-stream" in content_type:
        return StreamingResponse(
            _sse_body(upstream_response, mapping, endpoint, started),
            status_code=upstream_response.status_code,
            media_type="text/event-stream",
            headers=headers,
        )

    raw = await upstream_response.aread()
    await upstream_response.aclose()
    if mapping and "application/json" in content_type:
        try:
            value = json.loads(raw)
            _restore_output_calls(value, mapping)
            raw = json.dumps(value, separators=(",", ":")).encode()
        except json.JSONDecodeError:
            pass
    _metric_complete(endpoint, upstream_response.status_code, started)
    return Response(
        content=raw,
        status_code=upstream_response.status_code,
        headers=headers,
        media_type=None,
    )
