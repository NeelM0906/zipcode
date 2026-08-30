#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
docker compose -f "$script_dir/compose.yaml" ps
curl -fsS http://127.0.0.1:9090/-/ready
curl -fsS http://127.0.0.1:9090/api/v1/targets \
  | jq '{active: [.data.activeTargets[] | {job: .labels.job, instance: .labels.instance, health, lastError}]}'
curl -fsS http://127.0.0.1:3000/api/health | jq .
