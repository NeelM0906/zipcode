#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"
docker compose config --quiet
docker compose up -d
docker compose ps
