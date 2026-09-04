# Installation

ZIPCODE publishes native archives for Linux x86_64, macOS Apple Silicon,
macOS Intel, and Windows x86_64.

## macOS and Linux

```bash
curl -fsSL https://raw.githubusercontent.com/NeelM0906/zipcode/main/install.sh | sh
```

The default destination is `~/.local/bin`. Override it with
`ZIPCODE_INSTALL_DIR`:

```bash
curl -fsSL https://raw.githubusercontent.com/NeelM0906/zipcode/main/install.sh \
  | ZIPCODE_INSTALL_DIR=/usr/local/bin sh
```

Pin a release instead of installing the latest:

```bash
curl -fsSL https://raw.githubusercontent.com/NeelM0906/zipcode/main/install.sh \
  | ZIPCODE_VERSION=v0.2.0 sh
```

## Windows

Run in PowerShell:

```powershell
irm https://raw.githubusercontent.com/NeelM0906/zipcode/main/install.ps1 | iex
```

The installer uses `%USERPROFILE%\.local\bin` and adds that directory to your
user `PATH`. Open a new terminal after the first installation.

## First run

```bash
zip-code login
zip-code
```

Login requires an invitation for your GitHub username. ZIPCODE creates
`~/.zipcode/config.toml` on first launch and downloads the private model catalog
after successful authentication. The first coding-agent launch then displays a
full-trace collection notice. Type `I AGREE` to accept the current policy and
continue; the acceptance is stored in
`~/.zipcode/full-trace-consent.json`. Read [Trace collection and
storage](../TRACE_DATA.md) before accepting.

To disable both trace capture and upload, set
`ZIPCODE_DISABLE_TRACE_UPLOAD=1` in the environment before launching ZIPCODE.
This setting takes precedence over a previously saved acceptance.

## Verify the download manually

Each GitHub release includes `SHA256SUMS`. Download the archive and checksum
file from the same release, then run:

```bash
sha256sum -c SHA256SUMS --ignore-missing
```

The automated installers perform this check before copying any executable.

Release archives also carry signed GitHub/Sigstore build provenance. With the
GitHub CLI installed, verify a manually downloaded archive with:

```bash
gh attestation verify zipcode-x86_64-unknown-linux-gnu.tar.gz \
  --repo NeelM0906/zipcode
```

## Update

Rerun the installer. Authentication and local sessions remain in
`~/.zipcode`.

## Uninstall

Remove `zip-code`, `zip-code-core`, and `codex-code-mode-host` from the install
directory. Remove `~/.zipcode` only if you also want to delete local sessions,
configuration, fallback credentials, and locally retained trace bundles. This
does not delete bundles already uploaded to Supabase; contact the ZIPCODE
operator for remote deletion.
