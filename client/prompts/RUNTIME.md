# Runtime verification and skill discovery

Implemented in source, not installed or activated by default. The shared base-prompt
candidate remains on HOLD. These are independent changes, not an automatic
promotion of the shorter prompt.

## Verification adapter

`runtime_guard.py` uses existing lifecycle hooks and the ordinary shell tool.
It does not execute test commands from the privileged hook process. The agent
chooses a meaningful check, runs the `check -- <command> [args]` wrapper under
its existing sandbox/approvals, and the post-tool hook records the completed
receipt. Successful exit alone does not prove that a check covers the request.

- The turn starts with a source fingerprint. Changed source requires a fresh
  successful receipt before a verified completion.
- Fingerprints cover the Git revision and tracked modifications, staged changes,
  deletions, and non-ignored new files throughout the repository, including when
  the agent starts in a subdirectory. Ignored outputs are not covered.
- Subsequent source changes invalidate evidence. A check that changes source
  cannot certify its own result. A new/incomplete/failed check invalidates the
  previous success; missing receipts are not passes.
- Three identical failed checks against unchanged source deny a fourth attempt.
  Revised source or a different check can proceed. This applies to receipt-runner
  checks, not every arbitrary shell command.
- The stop hook permits one corrective continuation, then stops incomplete work.
  A specific `Verification: blocked — ...`, `Verification: failed — ...`, or
  `Verification: not run — ...` disclosure can finish without a retry loop.
- Bounds: 10,000 changed files, 64 MiB changed content, 1 MiB hook input,
  128 turn records, 16 pending checks/failure keys per record, 10-second hook
  deadline inside the engine's 15-second timeout, 2-second SQLite contention
  limit. No raw source, tool output, or credentials are stored in guard state.

This adapter is explicitly for **macOS/Linux POSIX shells in Git workspaces**.
It fails closed outside Git or when it cannot establish a bounded fingerprint.
It is a workflow guard, not an adversarial security boundary. It cannot certify
ignored files, external services, browser appearance, or the adequacy of a test.
Checks needing shell syntax can run `check -- /bin/sh -c '<command>'` through the
normal shell tool. Existing execution timeouts and permissions still apply.

### Activation (opt-in, after review)

`python3 client/prompts/guard_config.py --home /absolute/path/to/.zipcode`

This copies the adapter and creates hooks.json without overwriting any existing
hooks, adapter, state, config, auth, or model selection. Start ZIPCODE in a Git
repository and review/approve these exact hooks in `/hooks`; installation does
not bypass or pre-approve native hook trust. This is deliberately not enabled
globally on this Mac yet: starting ZIPCODE in a non-Git home directory would be
blocked by this adapter. Remove only these added hook entries to deactivate;
do not delete other user hooks or configuration.

## Skill discovery

`skills.list` accepts `authority: {kind: "host"}` and an optional case-insensitive
query of at most 256 bytes. It searches full names/descriptions before metadata
truncation and returns at most 20 entries per page within the host response
budget. Continue with the same query and returned cursor; changed queries or
catalogs reject stale cursors. Disabled/implicit-hidden skills remain excluded.

`skills.read` reads the selected host skill's main resource through its admitted
snapshot/provider, with existing pagination. Cross-package/unloaded paths are
rejected. Windows-normalized and logical display aliases match catalog rendering.
Host supporting files still use the normal host filesystem tools. Executor and
orchestrator authorities keep their existing behavior.

Each host `skills.list` and `skills.read` page has an 8,000-byte serialized-response ceiling,
including metadata and JSON escaping, even when called through Code Mode.
Smaller direct-call output budgets still apply. Continue with `next_cursor` to
read the whole skill; the cap does not silently discard its remaining content.
Large catalog entries use smaller pages. A single entry that cannot fit still
uses the existing explicit omission warning; executor/orchestrator caps are unchanged.
This reuses the existing skill-prompt byte ceiling, **not** an 8,000-token budget.
An individual page can still exceed 1,000 tokens, so this context-bearing change
requires the additional manual review specified by the repository guidelines.

The production step store now carries the admitted host snapshot before tool
construction; this is covered by a real core request/response integration test.
Two further real-agent tests require the Code Mode host executable and measure
every serialized list/read page under a 512 KiB model budget. They check complete
catalog pagination and exact reconstruction of a large Unicode/escaped skill
file through the production router, without manufacturing a `ToolCallSource`.
Inline catalog defaults remain unchanged until relevance-selection evaluation.
Host discovery tools respect the existing `include_instructions` opt-out, so
tool-free auxiliary requests do not accidentally acquire discovery tools.
Explicit skill mentions remain resolvable when that presentation flag is off.

## Validation and release boundaries

- Latest host-page follow-up: all 178 skill-extension tests and all three core
  discovery integration tests pass. The new real Code Mode list regression
  reproduced a 26,109-byte page before the cap; afterward both list and read
  remain at most 8,000 serialized bytes per page with complete pagination.
  Core tests require a built `codex-code-mode-host` executable (build it with
  `cargo build -p codex-code-mode-host` before a scoped Cargo-based core run).
  The full workspace suite was not repeated for this follow-up.
- PR review follow-up: 177 skill-extension tests and 28 Python tests pass after
  adding the host-read response cap and zombie-aware process assertions. Both
  new regression fixtures failed before their fixes. The Python suite now runs
  in Linux CI; its installed-CLI fixture remains opt-in via `ZIPCODE_TEST_CORE`.
- 180 focused Rust tests pass: 176 skill-extension tests, the core production
  discovery/read integration, a tool-free TUI recap regression, and two
  app-server orchestrator-isolation tests. Personal `~/.agents/skills` is
  read-isolated for this run because two existing host-service fixtures otherwise
  load unrelated global skills.
- 27 Python tests pass, including the opt-in installed-CLI integration with a
  local deterministic inference fixture and temporary approved hooks. It proves
  edit → unverified completion blocked → real check → completion accepted.
- The approved full workspace run executed 16,722 tests: 16,610 passed and 112
  failed, with 44 runner-skipped tests. Of 109 environment/unrelated failures,
  107 passed when rerun without nested Seatbelt isolation and with normal
  terminal-color settings. The three discovery-related failures are fixed and
  covered by the passing 180-test run above.
- Two full-suite failures remain: `user_shell_command_does_not_set_network_sandbox_env_var`
  inherits the harness's protected sandbox marker, and the V8 proof-of-concept
  `sandbox_feature_matches_linked_v8` test disagrees with workspace-unified V8
  sandbox features. These are not a clean full-suite release gate. Existing
  network-disabled early-return tests also limit live-network coverage.
- Scoped skill-extension/app-server Clippy and Python Ruff checks pass.
- Repository-wide `just fmt` completed; no tests were rerun after lint/format.
- Removed the 28 GiB temporary Rust build cache and 61 MiB failed-test checkout
  after validation. Saved prompt captures and test reports are retained.
- Validation did not install a rebuilt CLI or activate the guard globally.

Keep review stages separate: existing prompt/evaluation pack; native discovery
and its core wiring; the hook adapter and unit tests; then activation and the
installed-CLI integration/measurement test. Each follow-up stage is under the
800-line review limit; no monolithic release or commit is implied here.

## Prompt-size measurement

The candidate file measured 501 Qwen tokens; the installed baseline measured
3,674. Neither number is the whole request. In the September 7 installed-CLI
capture using this repository, the Qwen catalog and candidate override, the
effective trimmed base was 2,695 characters, surrounded by 27,686 other
developer-text characters, 23,515 repository/environment characters, and a
101-character inspection request. Tool declarations add 21,096 characters as
serialized JSON. These are measured **characters, not tokens**.
The Responses-lite path embeds base instructions and tool definitions in
developer messages; empty top-level `instructions`/`tools` does not mean absent
instructions or tools. The credential-free localhost capture preserves the
assembled request but is not an identical interactive-session packet or a
production-context token census.

The [Codex issue](https://github.com/openai/codex/issues/19212) quotes a model's
estimated breakdown after the author asked it, not an instrumented measurement
or an official required prompt budget. The shorter candidate's behavior and
total task cost—not a target prompt length—remain the release criteria.
