#!/usr/bin/env sh
set -eu

repo="NeelM0906/zipcode"
version="${ZIPCODE_VERSION:-latest}"
install_dir="${ZIPCODE_INSTALL_DIR:-${HOME}/.local/bin}"

case "$(uname -s):$(uname -m)" in
  Linux:x86_64) target="x86_64-unknown-linux-gnu" ;;
  Darwin:arm64) target="aarch64-apple-darwin" ;;
  Darwin:x86_64) target="x86_64-apple-darwin" ;;
  *)
    echo "ZIPCODE does not yet publish a binary for $(uname -s) $(uname -m)." >&2
    exit 1
    ;;
esac

for command in curl tar; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "ZIPCODE installation requires $command." >&2
    exit 1
  fi
done

if [ "$version" = "latest" ]; then
  release_url="https://github.com/${repo}/releases/latest/download"
else
  release_url="https://github.com/${repo}/releases/download/${version}"
fi
asset="zipcode-${target}.tar.gz"
temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT HUP INT TERM

echo "Downloading ZIPCODE for ${target}..."
curl -fL --retry 3 -o "${temporary}/${asset}" "${release_url}/${asset}"
curl -fL --retry 3 -o "${temporary}/SHA256SUMS" "${release_url}/SHA256SUMS"
expected="$(awk -v name="$asset" '$2 == name { print $1 }' "${temporary}/SHA256SUMS")"
if [ -z "$expected" ]; then
  echo "No checksum was published for ${asset}." >&2
  exit 1
fi
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "${temporary}/${asset}" | awk '{ print $1 }')"
else
  actual="$(shasum -a 256 "${temporary}/${asset}" | awk '{ print $1 }')"
fi
if [ "$actual" != "$expected" ]; then
  echo "ZIPCODE download checksum mismatch." >&2
  exit 1
fi

tar -xzf "${temporary}/${asset}" -C "$temporary"
mkdir -p "$install_dir"
for binary in zip-code zip-code-core codex-code-mode-host; do
  install -m 0755 "${temporary}/zipcode-${target}/${binary}" "${install_dir}/${binary}"
done

echo "ZIPCODE installed in ${install_dir}."
case ":${PATH}:" in
  *":${install_dir}:"*) ;;
  *) echo "Add ${install_dir} to PATH, then open a new terminal." ;;
esac
echo "Run: zip-code login"
