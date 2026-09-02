#!/usr/bin/env bash
set -euo pipefail

setup_version="1.1.0"
endpoint="https://notzipcode.ngrok.io/v1"
zipcode_home="${HOME}/.zipcode"
legacy_home="${HOME}/.qwen-codex"
launcher_dir="${HOME}/.local/bin"
launcher="${launcher_dir}/zip-code"
credential_file="${zipcode_home}/credential"
legacy_credential_file="${legacy_home}/credential"
path_line='export PATH="$HOME/.local/bin:$PATH"'

print_banner() {
  if [[ -t 1 ]] && [[ -z "${NO_COLOR:-}" ]]; then
    printf '\033[38;5;213m'
  fi
  printf '%s\n' \
    '███████╗██╗██████╗  ██████╗ ██████╗ ██████╗ ███████╗' \
    '╚══███╔╝██║██╔══██╗██╔════╝██╔═══██╗██╔══██╗██╔════╝' \
    '  ███╔╝ ██║██████╔╝██║     ██║   ██║██║  ██║█████╗  ' \
    ' ███╔╝  ██║██╔═══╝ ██║     ██║   ██║██║  ██║██╔══╝  ' \
    '███████╗██║██║     ╚██████╗╚██████╔╝██████╔╝███████╗' \
    '╚══════╝╚═╝╚═╝      ╚═════╝ ╚═════╝ ╚═════╝ ╚══════╝'
  if [[ -t 1 ]] && [[ -z "${NO_COLOR:-}" ]]; then
    printf '\033[0m'
  fi
  printf '%s\n' '             PRIVATE CODING AGENT'
}

if [[ "${1:-}" == "--version" ]]; then
  echo "ZIPCODE setup ${setup_version}"
  exit 0
fi

print_banner
echo
echo "Setting up ZIPCODE ${setup_version}..."

if ! command -v curl >/dev/null 2>&1; then
  echo "ZIPCODE setup needs curl. Install curl and run this file again." >&2
  exit 1
fi

export PATH="${HOME}/.local/bin:${PATH}"
if ! command -v codex >/dev/null 2>&1; then
  echo "Installing the official Codex CLI harness..."
  curl -fsSL https://chatgpt.com/codex/install.sh | sh
  export PATH="${HOME}/.local/bin:${PATH}"
fi

if ! command -v codex >/dev/null 2>&1; then
  echo "Codex installed, but it is not on PATH yet. Open a new terminal and run this setup again." >&2
  exit 1
fi

find_codex_native() {
  local entry resolved magic platform_package target package_json candidate
  entry="$(command -v codex)"
  resolved="$(perl -MCwd=abs_path -e "print abs_path(shift)" "${entry}")"
  magic="$(od -An -tx1 -N4 "${resolved}" | tr -d " \n")"
  case "${magic}" in
    7f454c46*|cffaedfe*|feedfacf*|cafebabe*|bebafeca*)
      printf "%s\n" "${resolved}"
      return 0
      ;;
  esac

  case "$(uname -s):$(uname -m)" in
    Darwin:arm64)
      platform_package="@openai/codex-darwin-arm64"
      target="aarch64-apple-darwin"
      ;;
    Darwin:x86_64)
      platform_package="@openai/codex-darwin-x64"
      target="x86_64-apple-darwin"
      ;;
    Linux:x86_64)
      platform_package="@openai/codex-linux-x64"
      target="x86_64-unknown-linux-musl"
      ;;
    Linux:aarch64|Linux:arm64)
      platform_package="@openai/codex-linux-arm64"
      target="aarch64-unknown-linux-musl"
      ;;
    *)
      return 1
      ;;
  esac
  command -v node >/dev/null 2>&1 || return 1
  package_json="$(node -e "
const fs = require(\"fs\");
const path = require(\"path\");
const { createRequire } = require(\"module\");
const entry = fs.realpathSync(process.argv[1]);
const request = createRequire(entry);
process.stdout.write(request.resolve(process.argv[2] + \"/package.json\"));
" "${resolved}" "${platform_package}" 2>/dev/null)" || return 1
  candidate="$(dirname "${package_json}")/vendor/${target}/bin/codex"
  [[ -x "${candidate}" ]] || return 1
  printf "%s\n" "${candidate}"
}

brand_codex_client() {
  local native native_dir host_source host_target brand_dir candidate branded old_title old_prompt new_title new_prompt
  if ! native="$(find_codex_native)"; then
    echo "ZIPCODE could not locate the native Codex executable to brand." >&2
    return 1
  fi
  command -v perl >/dev/null 2>&1 || {
    echo "ZIPCODE setup needs perl to prepare the branded client." >&2
    return 1
  }
  native_dir="$(dirname "${native}")"
  host_source="${native_dir}/codex-code-mode-host"
  host_target="${zipcode_home}/bin/codex-code-mode-host"
  brand_dir="${zipcode_home}/bin"
  branded="${brand_dir}/zipcode-client"
  candidate="${brand_dir}/zipcode-client.new"
  mkdir -p "${brand_dir}"
  install -m 0755 "${native}" "${candidate}"
  if [[ -x "${host_source}" ]]; then
    install -m 0755 "${host_source}" "${host_target}"
  fi

  old_title="$(NEEDLE="OpenAI Codex" perl -0777 -ne '$c=()=/\Q$ENV{NEEDLE}\E/g; print $c' "${candidate}")"
  old_prompt="$(NEEDLE="Ask Codex to do " perl -0777 -ne '$c=()=/\Q$ENV{NEEDLE}\E/g; print $c' "${candidate}")"
  if [[ "${old_title}" -gt 0 ]] && [[ "${old_prompt}" -gt 0 ]]; then
    perl -0777 -pi -e "s/OpenAI Codex/ZIPCODE     /g; s/Ask Codex to do /Ask ZIPCODE: do /g; s/Build faster with Codex\./Build fast with ZIPCODE./g; s/Codex\x27s Linux sandbox/ZIPCODE Linux sandbox/g; s/Access legacy models by running codex -m/Access legacy models using zip-code -m  /g" "${candidate}"
  fi

  old_title="$(NEEDLE="OpenAI Codex" perl -0777 -ne '$c=()=/\Q$ENV{NEEDLE}\E/g; print $c' "${candidate}")"
  old_prompt="$(NEEDLE="Ask Codex to do " perl -0777 -ne '$c=()=/\Q$ENV{NEEDLE}\E/g; print $c' "${candidate}")"
  new_title="$(NEEDLE="ZIPCODE     " perl -0777 -ne '$c=()=/\Q$ENV{NEEDLE}\E/g; print $c' "${candidate}")"
  new_prompt="$(NEEDLE="Ask ZIPCODE: do " perl -0777 -ne '$c=()=/\Q$ENV{NEEDLE}\E/g; print $c' "${candidate}")"
  if [[ "${old_title}" != 0 ]] || [[ "${old_prompt}" != 0 ]] || [[ "${new_title}" -lt 1 ]] || [[ "${new_prompt}" -lt 1 ]]; then
    echo "ZIPCODE branding guard failed; the installed Codex build is not compatible with this setup version." >&2
    return 1
  fi
  if [[ "$(uname -s)" == "Darwin" ]]; then
    codesign --sign - --force --preserve-metadata=identifier,entitlements,flags,runtime "${candidate}" >/dev/null 2>&1 || {
      echo "ZIPCODE could not ad-hoc sign the branded macOS client." >&2
      return 1
    }
    codesign --verify --verbose=2 "${candidate}" >/dev/null 2>&1 || {
      echo "ZIPCODE could not verify the branded macOS client signature." >&2
      return 1
    }
  fi
  "${candidate}" --version >/dev/null
  mv -f "${candidate}" "${branded}"
  chmod 0755 "${branded}"
}

mkdir -p "${zipcode_home}" "${launcher_dir}"
chmod 0700 "${zipcode_home}"
umask 077
brand_codex_client

migrated_credential=0
if [[ -n "${ZIPCODE_API_KEY:-}" ]]; then
  credential="${ZIPCODE_API_KEY}"
elif [[ -n "${QWEN38_API_KEY:-}" ]]; then
  credential="${QWEN38_API_KEY}"
elif [[ -s "${credential_file}" ]]; then
  IFS= read -r credential < "${credential_file}"
  echo "Using the saved ZIPCODE credential."
elif [[ -s "${legacy_credential_file}" ]]; then
  IFS= read -r credential < "${legacy_credential_file}"
  migrated_credential=1
  echo "Found your previous installation; migrating its saved credential."
else
  read -rsp "Paste the private team credential: " credential
  echo
fi

if [[ -z "${credential}" ]] || [[ "${credential}" == *$'\n'* ]] || [[ "${credential}" == *$'\r'* ]]; then
  echo "The credential is empty or invalid." >&2
  exit 1
fi

cat > "${zipcode_home}/config.toml" <<'CONFIG'
model = "Qwen/Qwen3.8-Flash-Next"
model_provider = "zipcode_team"
model_reasoning_effort = "xhigh"
model_reasoning_summary = "none"
approval_policy = "on-request"
sandbox_mode = "workspace-write"

[model_providers.zipcode_team]
name = "ZIPCODE Private Coding Cloud"
base_url = "https://notzipcode.ngrok.io/v1"
env_key = "ZIPCODE_API_KEY"
wire_api = "responses"
request_max_retries = 2
stream_max_retries = 2
stream_idle_timeout_ms = 1800000

[features]
apps = false
browser_use = false
computer_use = false
image_generation = false
multi_agent = false
plugins = false
CONFIG
chmod 0600 "${zipcode_home}/config.toml"

cat > "${launcher}" <<'LAUNCHER'
#!/usr/bin/env bash
set -euo pipefail

zipcode_home="${HOME}/.zipcode"
credential_file="${zipcode_home}/credential"
endpoint="https://notzipcode.ngrok.io/v1"

print_banner() {
  if [[ -t 1 ]] && [[ -z "${NO_COLOR:-}" ]]; then
    printf '\033[38;5;213m'
  fi
  printf '%s\n' \
    '███████╗██╗██████╗  ██████╗ ██████╗ ██████╗ ███████╗' \
    '╚══███╔╝██║██╔══██╗██╔════╝██╔═══██╗██╔══██╗██╔════╝' \
    '  ███╔╝ ██║██████╔╝██║     ██║   ██║██║  ██║█████╗  ' \
    ' ███╔╝  ██║██╔═══╝ ██║     ██║   ██║██║  ██║██╔══╝  ' \
    '███████╗██║██║     ╚██████╗╚██████╔╝██████╔╝███████╗' \
    '╚══════╝╚═╝╚═╝      ╚═════╝ ╚═════╝ ╚═════╝ ╚══════╝'
  if [[ -t 1 ]] && [[ -z "${NO_COLOR:-}" ]]; then
    printf '\033[0m'
  fi
  printf '%s\n' '             PRIVATE CODING AGENT'
}

if [[ "${1:-}" == "--welcome" ]]; then
  print_banner
  exit 0
fi

requested_model="${ZIPCODE_MODEL:-${QWEN_CODEX_MODEL:-flash}}"
case "${requested_model}" in
  flash|Qwen/Qwen3.8-Flash-Next|zipcode-flash|qwen-codex-flash-next)
    model="Qwen/Qwen3.8-Flash-Next"
    mode_label="QWEN3.8 FLASH-NEXT · 524K context · xhigh reasoning"
    ;;
  full|Qwen/Qwen3.8-27B-FP8|zipcode-full)
    model="Qwen/Qwen3.8-27B-FP8"
    mode_label="QWEN3.8-27B FP8 · 1M context · xhigh reasoning"
    ;;
  *)
    echo "Unknown ZIPCODE_MODEL. Use 'flash' (default) or 'full'." >&2
    exit 2
    ;;
esac

if [[ ! -x "${zipcode_home}/bin/zipcode-client" ]]; then
  echo "ZIPCODE client is missing. Rerun zip-code-setup.sh." >&2
  exit 127
fi
if ! command -v curl >/dev/null 2>&1; then
  echo "ZIPCODE needs curl for its connection check." >&2
  exit 127
fi

if [[ -n "${ZIPCODE_API_KEY:-}" ]]; then
  credential="${ZIPCODE_API_KEY}"
elif [[ -s "${credential_file}" ]]; then
  IFS= read -r credential < "${credential_file}"
else
  read -rsp "Paste the private team credential: " credential
  echo
fi

if [[ -z "${credential}" ]]; then
  echo "ZIPCODE authentication failed: no credential was supplied." >&2
  exit 1
fi

response_file="$(mktemp)"
cleanup() {
  rm -f -- "${response_file}"
}
trap cleanup EXIT
http_code="$(curl --silent --show-error --max-time 30 \
  --output "${response_file}" --write-out '%{http_code}' \
  --header "Authorization: Bearer ${credential}" \
  "${endpoint}/models?client_version=0.150.1")"
if [[ "${http_code}" != "200" ]] || ! grep -Fq "\"slug\":\"${model}\"" "${response_file}"; then
  echo "ZIPCODE could not connect (HTTP ${http_code}). The team credential may have rotated." >&2
  echo "Rerun zip-code-setup.sh to repair the installation." >&2
  exit 1
fi

if [[ ! -s "${credential_file}" ]]; then
  umask 077
  printf '%s\n' "${credential}" > "${credential_file}"
  chmod 0600 "${credential_file}"
fi
install -m 0644 "${response_file}" "${zipcode_home}/models.json"
rm -f -- "${response_file}"
trap - EXIT

launch_args=()
if [[ "$#" -eq 0 ]]; then
  print_banner
  echo
  printf '  %-11s %s\n' 'STATUS' 'CONNECTED'
  printf '  %-11s %s\n' 'MODE' "${mode_label}"
  printf '  %-11s %s\n' 'WORKSPACE' "${PWD}"
  echo
  echo "  /model switches between Qwen3.8 Flash-Next and Qwen3.8-27B"
  echo
  # Inline TUI mode preserves the welcome screen in terminal scrollback.
  launch_args=(--no-alt-screen)
fi

export ZIPCODE_API_KEY="${credential}"
export CODEX_HOME="${zipcode_home}"
catalog_args=(-c "model_catalog_json=\"${zipcode_home}/models.json\"")
exec "${zipcode_home}/bin/zipcode-client" --strict-config "${catalog_args[@]}" \
  -c "model=\"${model}\"" \
  -c 'model_provider="zipcode_team"' \
  -c 'model_reasoning_effort="xhigh"' \
  "${launch_args[@]}" \
  "$@"
LAUNCHER
chmod 0755 "${launcher}"

# Keep the old command as a clear migration signpost. It deliberately does not
# launch the product so every user learns the single supported command.
cat > "${launcher_dir}/qwen-codex" <<'LEGACY'
#!/usr/bin/env bash
echo "Qwen Codex is now ZIPCODE." >&2
echo "Open a new terminal and type: zip-code" >&2
exit 2
LEGACY
chmod 0755 "${launcher_dir}/qwen-codex"

catalog_candidate="${zipcode_home}/models.json.candidate"
cleanup_catalog_candidate() {
  rm -f -- "${catalog_candidate}"
}
trap cleanup_catalog_candidate EXIT
http_code="$(curl --silent --show-error --max-time 30 \
  --output "${catalog_candidate}" --write-out '%{http_code}' \
  --header "Authorization: Bearer ${credential}" \
  "${endpoint}/models?client_version=0.150.1")"
if [[ "${http_code}" != "200" ]] || \
   ! grep -Fq '"slug":"Qwen/Qwen3.8-Flash-Next"' "${catalog_candidate}" || \
   ! grep -Fq '"slug":"Qwen/Qwen3.8-27B-FP8"' "${catalog_candidate}"; then
  echo "The server rejected that credential or returned an old catalog (HTTP ${http_code})." >&2
  exit 1
fi
install -m 0644 "${catalog_candidate}" "${zipcode_home}/models.json"
rm -f -- "${catalog_candidate}"
trap - EXIT

printf '%s\n' "${credential}" > "${credential_file}"
chmod 0600 "${credential_file}"

profile_files=("${HOME}/.profile")
case "$(basename -- "${SHELL:-bash}")" in
  zsh) profile_files+=("${HOME}/.zshrc") ;;
  bash) profile_files+=("${HOME}/.bashrc" "${HOME}/.bash_profile") ;;
esac
for profile_file in "${profile_files[@]}"; do
  touch "${profile_file}"
  if ! grep -Fqx "${path_line}" "${profile_file}"; then
    printf '\n%s\n' "${path_line}" >> "${profile_file}"
  fi
done
export PATH="${launcher_dir}:${PATH}"

CODEX_HOME="${zipcode_home}" "${zipcode_home}/bin/zipcode-client" --strict-config \
  -c "model_catalog_json=\"${zipcode_home}/models.json\"" --version >/dev/null

echo
if [[ "${migrated_credential}" == "1" ]]; then
  echo "Your previous credential was migrated. The old folder was left intact as a backup."
fi
echo "ZIPCODE is ready."
echo
echo "  1. Open a new terminal"
echo "  2. Go to any project"
echo "  3. Type: zip-code"
echo
echo "Need the 1M model for one session? Type: ZIPCODE_MODEL=full zip-code"

if [[ "${ZIPCODE_OPEN_NOW:-0}" == "1" ]]; then
  exec "${launcher}"
fi
