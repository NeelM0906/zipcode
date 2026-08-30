#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"
docker compose ps
printf '\nObservability services:\n'
docker compose --project-directory ../observability -f ../observability/compose.yaml ps
printf '\nInternal router models:\n'
curl -fsS --max-time 5 http://127.0.0.1:8003/v1/models || true
printf '\n\nCodex gateway health:\n'
curl -fsS --max-time 5 http://127.0.0.1:8012/health || true
printf '\n\nCodex model catalog:\n'
curl -fsS --max-time 5 'http://127.0.0.1:8012/v1/models?client_version=0.150.1' \
  | jq '{models: [.models[] | {slug, display_name, default_reasoning_level, context_window, max_context_window, supports_search_tool}]}' || true
printf '\n\nGPU allocation:\n'
nvidia-smi --query-gpu=index,uuid,memory.used,memory.free,utilization.gpu --format=csv,noheader
printf '\nPrometheus Qwen targets:\n'
curl -fsS --max-time 5 http://127.0.0.1:9090/api/v1/targets \
  | jq '{targets: [.data.activeTargets[] | select(.labels.job | startswith("qwen")) | {job: .labels.job, instance: .labels.instance, health, lastError}]}' || true
