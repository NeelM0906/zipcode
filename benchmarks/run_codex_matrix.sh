#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
for concurrency in 1 2 4 8 16; do
    "$script_dir/codex_agent_bench.py" \
        --concurrency "$concurrency" \
        --tasks "$concurrency"
done
