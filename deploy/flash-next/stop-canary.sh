#!/usr/bin/env bash
set -euo pipefail

LAB_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# Keep the public model mux on port 8002. With Flash-Next unavailable it
# advertises and routes only the full 27B model through the gateway on 8012.
docker compose --project-directory "$LAB_ROOT" stop codex-gateway model
docker start qwen38-gpu1 >/dev/null
echo "Flash-Next stopped, the public mux remains live, and the GPU 1 Qwen3.8-27B replica was restored."
