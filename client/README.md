# ZIPCODE team coding agent

ZIPCODE is the team's private coding agent running on the Codex CLI harness. It
uses the authenticated endpoint at `https://olympustest.ngrok.pro/v1` and keeps
its configuration isolated in `~/.zipcode`, so the ordinary `codex` command and
the user's OpenAI model setup remain untouched.

## The entire teammate flow

Send the teammate `zip-code-setup.sh` and the team credential through separate,
private channels. They run the setup file once, paste the credential, open a new
terminal, enter a project, and type:

```bash
zip-code
```

The launcher displays the ZIPCODE welcome screen, verifies the private
connection, refreshes the model catalog, and opens the coding agent. `/model`
shows only:

- `Qwen/Qwen3.8-Flash-Next` — Qwen3.8 Flash-Next NVFP4, 524K context, optimized for fast
  interactive work.
- `Qwen/Qwen3.8-27B-FP8` — Qwen3.8-27B FP8, 1M context, for exceptional large-context
  sessions.

The setup works for fresh macOS/Linux systems, repairs current ZIPCODE installs,
and migrates a credential from the previous `~/.qwen-codex` install. It leaves
that old directory intact as a backup. The old `qwen-codex` command becomes a
migration message so `zip-code` is the single supported entry point.

To start directly on the 1M model for one session:

```bash
ZIPCODE_MODEL=full zip-code
```

To preview the welcome screen without opening Codex:

```bash
zip-code --welcome
```

## What setup changes

- Installs the official Codex CLI harness if it is missing, then creates a locally branded ZIPCODE client with guarded binary-string checks and macOS ad-hoc signing.
- Creates `~/.zipcode/config.toml`, `models.json`, and a mode-`0600` credential.
- Installs `~/.local/bin/zip-code` and adds that directory to the user's shell
  `PATH` idempotently.
- Uses the Responses API, `xhigh` reasoning, `workspace-write`, and `on-request`
  approval defaults.
- Disables unrelated apps, browser/computer tools, plugins, image generation,
  and multi-agent features for the qualified coding path.

The model picker and session header retain the real Qwen repository names. ZIPCODE is the product and TUI identity, while the underlying models remain explicitly Qwen.

## Security and capacity

Do not put the credential in TOML, source control, screenshots, or shell history.
The current pilot credential is shared, so a leak requires team-wide rotation
and requests are not attributable per user.

Flash-Next is qualified for four simultaneous active requests on GPU 1; the
four-agent correctness run passed 4/4 at 316.12 aggregate output tokens/s. The
27B route runs on GPU 0. Treat 1M as a ceiling for exceptional tasks, not the
normal low-latency operating point.

## Troubleshooting

- Only GPT models appear: close that session and launch with `zip-code`, not
  `codex`.
- `401` or “credential may have rotated”: rerun `zip-code-setup.sh` and enter the
  current credential.
- Command not found: open a new terminal or add `~/.local/bin` to `PATH`.
- Run `~/.zipcode/verify.sh` when using the archive-based operator kit.

Validated with Codex CLI 0.150.1 on 2026-08-28.
