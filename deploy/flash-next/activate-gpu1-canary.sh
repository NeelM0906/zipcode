#!/usr/bin/env bash
set -euo pipefail

LAB_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
if [[ -f "$LAB_ROOT/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  source "$LAB_ROOT/.env"
  set +a
fi
MODEL_ROOT="${FLASH_NEXT_MODEL_PATH:?set FLASH_NEXT_MODEL_PATH in $LAB_ROOT/.env}"
HF="${HF_BIN:-$(command -v hf || true)}"
OLD_CONTAINER=qwen38-gpu1

for required in config.json model.safetensors.index.json chat_template.jinja; do
  [[ -s "$MODEL_ROOT/$required" ]] || {
    echo "Checkpoint is incomplete: missing $MODEL_ROOT/$required" >&2
    exit 1
  }
done

[[ -n "$HF" && -x "$HF" ]] || { echo "Missing Hugging Face CLI; install hf or set HF_BIN." >&2; exit 1; }
echo "Verifying the pinned checkpoint before touching a live GPU..."
HF_HOME="${FLASH_NEXT_CACHE_ROOT:?set FLASH_NEXT_CACHE_ROOT in $LAB_ROOT/.env}/huggingface" "$HF" cache verify \
  RadixArk/Qwen3.8-Flash-Next-NVFP4 \
  --revision 7b719225242aacd3dbd3f9407468c2ee9a9d2594 \
  --local-dir "$MODEL_ROOT" \
  --fail-on-missing-files

docker compose --project-directory "$LAB_ROOT" config --quiet

old_was_running=false
if [[ "$(docker inspect -f '{{.State.Running}}' "$OLD_CONTAINER" 2>/dev/null || true)" == true ]]; then
  old_was_running=true
  echo "Draining the GPU 1 Qwen3.8-27B replica..."
  docker stop --time 120 "$OLD_CONTAINER"
fi

rollback() {
  echo "Flash-Next failed to become healthy; restoring the GPU 1 27B replica." >&2
  # Keep the public model mux alive so it can continue routing the full model.
  docker compose --project-directory "$LAB_ROOT" stop codex-gateway model || true
  if [[ "$old_was_running" == true ]]; then
    docker start "$OLD_CONTAINER" || true
  fi
}
trap rollback ERR

docker compose --project-directory "$LAB_ROOT" up -d model
deadline=$((SECONDS + 1800))
until curl -fsS --max-time 5 http://127.0.0.1:8020/health >/dev/null; do
  if (( SECONDS >= deadline )); then
    docker logs --tail 300 qwen38-flash-next-gpu1 >&2 || true
    exit 1
  fi
  if [[ "$(docker inspect -f '{{.State.Running}}' qwen38-flash-next-gpu1 2>/dev/null || true)" != true ]]; then
    docker logs --tail 300 qwen38-flash-next-gpu1 >&2 || true
    exit 1
  fi
  sleep 10
done

docker compose --project-directory "$LAB_ROOT" up -d codex-gateway
gateway_deadline=$((SECONDS + 120))
until curl -fsS --max-time 5 http://127.0.0.1:8022/health; do
  if (( SECONDS >= gateway_deadline )); then
    docker logs --tail 200 qwen38-flash-next-codex-gateway >&2 || true
    exit 1
  fi
  if [[ "$(docker inspect -f '{{.State.Running}}' qwen38-flash-next-codex-gateway 2>/dev/null || true)" != true ]]; then
    docker logs --tail 200 qwen38-flash-next-codex-gateway >&2 || true
    exit 1
  fi
  sleep 2
done
trap - ERR
echo
echo "Flash-Next canary is healthy: http://127.0.0.1:8022/v1"
