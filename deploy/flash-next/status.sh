#!/usr/bin/env bash
set -euo pipefail

LAB_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
docker compose --project-directory "$LAB_ROOT" ps
curl -fsS --max-time 5 http://127.0.0.1:8020/v1/models \
  | jq '{engine_models: [.data[] | {id, max_model_len}]}' || true
echo
curl -fsS --max-time 5 'http://127.0.0.1:8022/v1/models?client_version=0.150.1' \
  | jq '{flash_catalog: [.models[] | {slug, context_window, tool_mode}]}' || true
echo
curl -fsS --max-time 5 http://127.0.0.1:8002/health || true
echo
curl -fsS --max-time 5 'http://127.0.0.1:8002/v1/models?client_version=0.150.1' \
  | jq '{team_catalog: [.models[] | {slug, context_window, tool_mode}]}' || true
echo
nvidia-smi --query-gpu=index,uuid,memory.used,memory.free,utilization.gpu --format=csv,noheader
