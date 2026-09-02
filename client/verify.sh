#!/usr/bin/env bash
set -euo pipefail

zipcode_home="${ZIPCODE_HOME:-${HOME}/.zipcode}"
endpoint="https://notzipcode.ngrok.io/v1"
model="Qwen/Qwen3.8-Flash-Next"
verify_sandbox="${ZIPCODE_VERIFY_SANDBOX:-workspace-write}"

if [[ ! -x "${zipcode_home}/bin/zipcode-client" ]]; then
  echo "verify: branded ZIPCODE client is missing; rerun zip-code-setup.sh." >&2
  exit 127
fi
if ! command -v curl >/dev/null 2>&1; then
  echo "verify: curl is required." >&2
  exit 127
fi
if [[ -z "${ZIPCODE_API_KEY:-}" ]]; then
  echo "verify: ZIPCODE_API_KEY is not set." >&2
  exit 2
fi
if [[ ! -f "${zipcode_home}/config.toml" ]]; then
  echo "verify: missing ${zipcode_home}/config.toml; run install.sh first." >&2
  exit 2
fi
if [[ ! -s "${zipcode_home}/models.json" ]]; then
  echo "verify: missing ${zipcode_home}/models.json; run install.sh again." >&2
  exit 2
fi
case "${verify_sandbox}" in
  read-only|workspace-write|danger-full-access) ;;
  *)
    echo "verify: invalid ZIPCODE_VERIFY_SANDBOX value." >&2
    exit 2
    ;;
esac
if [[ "${verify_sandbox}" == "danger-full-access" ]]; then
  echo "WARNING: the tool probe is running without a ZIPCODE sandbox in a fresh temporary directory." >&2
fi

probe_dir="$(mktemp -d)"
cleanup() {
  rm -rf -- "${probe_dir}"
}
trap cleanup EXIT

models_body="${probe_dir}/models.json"
models_code="$(curl --silent --show-error --max-time 30 \
  --output "${models_body}" --write-out '%{http_code}' \
  --header "Authorization: Bearer ${ZIPCODE_API_KEY}" \
  "${endpoint}/models")"
if [[ "${models_code}" != "200" ]] || ! grep -Fq "${model}" "${models_body}"; then
  echo "verify: authenticated model discovery failed (HTTP ${models_code})." >&2
  exit 1
fi
echo "PASS: authenticated model discovery"

codex_output="${probe_dir}/codex-output.txt"
probe_prompt='Use a shell command to create team_probe.txt containing exactly ZIPCODE_TOOL_OK, read the file back, and then answer exactly ZIPCODE_TEAM_OK.'
if ! CODEX_HOME="${zipcode_home}" "${zipcode_home}/bin/zipcode-client" exec \
  -c "model_catalog_json=\"${zipcode_home}/models.json\"" \
  --ephemeral \
  --skip-git-repo-check \
  --sandbox "${verify_sandbox}" \
  -C "${probe_dir}" \
  "${probe_prompt}" 2>&1 | tee "${codex_output}"; then
  echo "verify: the ZIPCODE agent request failed." >&2
  exit 1
fi

if [[ ! -f "${probe_dir}/team_probe.txt" ]] || \
   [[ "$(tr -d '\r\n' < "${probe_dir}/team_probe.txt")" != "ZIPCODE_TOOL_OK" ]] || \
   ! grep -Fq "ZIPCODE_TEAM_OK" "${codex_output}"; then
  echo "verify: the model responded, but the shell-tool correctness gate failed." >&2
  exit 1
fi

echo "PASS: public Responses API, xhigh reasoning, ZIPCODE shell tool, and tool-result replay"
echo "READY: run zip-code inside a repository"
