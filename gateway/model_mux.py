"""Small model-aware front door for the local Codex gateways."""

from __future__ import annotations

import json
import os
from copy import deepcopy
from collections.abc import AsyncIterator

import httpx
from fastapi import FastAPI, Request
from fastapi.responses import (
    FileResponse,
    JSONResponse,
    PlainTextResponse,
    Response,
    StreamingResponse,
)

try:
    from .auth import authenticate_request, router as auth_router
except ImportError:
    from auth import authenticate_request, router as auth_router


FLASH_MODEL = os.environ.get("FLASH_MODEL", "qwen-codex-flash-next")
FLASH_ALIAS = os.environ.get("FLASH_ALIAS", "Qwen/Qwen3.8-Flash-Next")
FLASH_UPSTREAM = os.environ.get("FLASH_UPSTREAM", "http://127.0.0.1:8022").rstrip("/")
FULL_MODEL = os.environ.get("FULL_MODEL", "Qwen/Qwen3.8-27B-FP8")
FULL_ALIAS = os.environ.get("FULL_ALIAS", "Qwen/Qwen3.8-27B-FP8")
FULL_UPSTREAM = os.environ.get("FULL_UPSTREAM", "http://127.0.0.1:8012").rstrip("/")
SETUP_SCRIPT_PATH = os.environ.get(
    "SETUP_SCRIPT_PATH", "/mux/zip-code-setup.sh"
)
BRANDED_ROUTES = {
    FLASH_ALIAS: (FLASH_UPSTREAM, FLASH_MODEL),
    FULL_ALIAS: (FULL_UPSTREAM, FULL_MODEL),
}
# Previously issued ZIPCODE aliases and the Flash gateway's internal serving
# name remain accepted during migration, but are not advertised in /model.
LEGACY_ROUTES = {
    FLASH_MODEL: (FLASH_UPSTREAM, FLASH_MODEL),
    "zipcode-flash": (FLASH_UPSTREAM, FLASH_MODEL),
    "zipcode-full": (FULL_UPSTREAM, FULL_MODEL),
}
ROUTES = {**BRANDED_ROUTES, **LEGACY_ROUTES}
CATALOG_ROUTES = (
    (FLASH_UPSTREAM, FLASH_MODEL, FLASH_ALIAS, "Qwen3.8 Flash-Next NVFP4", "Qwen3.8 Flash-Next NVFP4 coding model; 524K qualified context."),
    (FULL_UPSTREAM, FULL_MODEL, FULL_ALIAS, "Qwen3.8-27B FP8", "Qwen3.8-27B FP8 coding model; 1M context."),
)

app = FastAPI(title="ZIPCODE private model gateway", version="2.0.0")
app.include_router(auth_router)
client: httpx.AsyncClient | None = None


def _brand_catalog_entry(
    entry: dict,
    *,
    alias: str,
    display_name: str,
    description: str,
    codex_catalog: bool,
) -> dict:
    branded = deepcopy(entry)
    branded["slug" if codex_catalog else "id"] = alias
    if codex_catalog:
        branded["display_name"] = display_name
        branded["description"] = description
        messages = branded.get("model_messages")
        if isinstance(messages, dict):
            instructions = messages.get("instructions_template")
            if isinstance(instructions, str):
                instructions = instructions.replace(
                    "You are Codex, powered by the local Qwen3.8 Flash-Next model.",
                    "You are ZIPCODE, a private coding agent powered by Qwen3.8 Flash-Next.",
                    1,
                ).replace(
                    "You are Codex, powered by the local Qwen3.8-27B model.",
                    "You are ZIPCODE, a private coding agent powered by Qwen3.8-27B.",
                    1,
                )
                messages["instructions_template"] = instructions
    return branded


def _forward_headers(request: Request) -> dict[str, str]:
    excluded = {"host", "content-length", "connection", "transfer-encoding", "authorization"}
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


async def _stream(response: httpx.Response) -> AsyncIterator[bytes]:
    try:
        async for chunk in response.aiter_raw():
            yield chunk
    finally:
        await response.aclose()


@app.on_event("startup")
async def startup() -> None:
    global client
    client = httpx.AsyncClient(timeout=None, limits=httpx.Limits(max_connections=512))


@app.on_event("shutdown")
async def shutdown() -> None:
    if client is not None:
        await client.aclose()


@app.get("/health")
@app.get("/v1/health")
async def health() -> JSONResponse:
    assert client is not None
    states: dict[str, bool] = {}
    for model, (upstream, _) in BRANDED_ROUTES.items():
        try:
            response = await client.get(f"{upstream}/health", timeout=5)
            states[model] = response.is_success
        except httpx.HTTPError:
            states[model] = False
    # The front door stays healthy when at least one route is usable; per-model
    # status remains visible so maintenance on one GPU does not hide failures.
    return JSONResponse(
        {"status": "ok" if any(states.values()) else "unavailable", "models": states},
        status_code=200 if any(states.values()) else 503,
    )


@app.get("/v1/models")
async def models(request: Request) -> JSONResponse:
    authenticate_request(request)
    assert client is not None
    codex_catalog = bool(request.query_params.get("client_version"))
    combined: list[dict] = []
    for upstream, actual_model, alias, display_name, description in CATALOG_ROUTES:
        try:
            response = await client.get(
                f"{upstream}/v1/models",
                params=dict(request.query_params),
                headers=_forward_headers(request),
                timeout=10,
            )
            response.raise_for_status()
            payload = response.json()
            entries = payload.get("models" if codex_catalog else "data") or []
            key = "slug" if codex_catalog else "id"
            entry = next((item for item in entries if item.get(key) == actual_model), None)
            if entry is None and len(entries) == 1:
                entry = entries[0]
            if entry is not None:
                combined.append(
                    _brand_catalog_entry(
                        entry,
                        alias=alias,
                        display_name=display_name,
                        description=description,
                        codex_catalog=codex_catalog,
                    )
                )
        except (httpx.HTTPError, ValueError):
            continue
    if codex_catalog:
        return JSONResponse({"models": combined})
    return JSONResponse({"object": "list", "data": combined})


@app.get("/install/zip-code-setup.sh")
@app.get("/v1/install/zip-code-setup.sh")
@app.get("/install/qwen-codex-setup.sh")
@app.get("/v1/install/qwen-codex-setup.sh")
async def setup_script() -> FileResponse:
    return FileResponse(
        SETUP_SCRIPT_PATH,
        media_type="text/x-shellscript",
        filename="zip-code-setup.sh",
    )


@app.get("/install/zip-code-setup.sha256")
@app.get("/v1/install/zip-code-setup.sha256")
@app.get("/install/qwen-codex-setup.sha256")
@app.get("/v1/install/qwen-codex-setup.sha256")
async def setup_script_sha256() -> PlainTextResponse:
    import hashlib

    with open(SETUP_SCRIPT_PATH, "rb") as script:
        digest = hashlib.file_digest(script, "sha256").hexdigest()
    return PlainTextResponse(f"{digest}  zip-code-setup.sh\n")


@app.api_route("/{path:path}", methods=["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"])
async def proxy(path: str, request: Request) -> Response:
    assert client is not None
    authenticate_request(request)
    body = await request.body()
    model = None
    if body and request.headers.get("content-type", "").startswith("application/json"):
        try:
            payload = json.loads(body)
            if isinstance(payload, dict):
                model = payload.get("model")
        except json.JSONDecodeError:
            pass
    if model is not None and model not in ROUTES:
        return JSONResponse(
            {"error": {"message": f"Unknown local model: {model}"}}, status_code=400
        )
    upstream, upstream_model = ROUTES.get(
        model, (FLASH_UPSTREAM, FLASH_MODEL)
    )
    if model in BRANDED_ROUTES:
        payload["model"] = upstream_model
        body = json.dumps(payload, separators=(",", ":")).encode()
    url = f"{upstream}/{path}"
    if request.url.query:
        url += f"?{request.url.query}"
    upstream_request = client.build_request(
        request.method,
        url,
        headers=_forward_headers(request),
        content=body,
    )
    try:
        response = await client.send(upstream_request, stream=True)
    except httpx.HTTPError:
        return JSONResponse(
            {"error": {"message": f"ZIPCODE inference route unavailable for {model or FLASH_ALIAS}"}},
            status_code=502,
        )
    headers = _response_headers(response)
    if "text/event-stream" in response.headers.get("content-type", ""):
        return StreamingResponse(
            _stream(response),
            status_code=response.status_code,
            media_type="text/event-stream",
            headers=headers,
        )
    content = await response.aread()
    await response.aclose()
    return Response(content=content, status_code=response.status_code, headers=headers)
