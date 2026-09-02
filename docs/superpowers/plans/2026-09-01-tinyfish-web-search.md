# TinyFish Web Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make ZIPCODE's existing standalone `web.run` tool execute live web searches through TinyFish for Qwen/custom model providers.

**Architecture:** Add an explicit provider selector to `[tools.web_search]`, keep the legacy model-provider search backend as the default, and add a focused TinyFish REST client inside `codex-rs/ext/web-search`. The TinyFish provider reuses the existing `web.run` registration and web-search events but exposes a search-only schema and sends only explicit search parameters to TinyFish.

**Tech Stack:** Rust 1.95, Tokio, serde/schemars, ZIPCODE extension APIs, `codex-http-client`, wiremock, cargo-nextest through `just test`.

**Spec:** `docs/superpowers/specs/2026-09-01-tinyfish-web-search-design.md`

## Global Constraints

- Phase 1 supports TinyFish Search only; do not add Fetch, browser automation, page opening, clicking, screenshots, or vertical lookup tools.
- Production TinyFish Search endpoint is exactly `https://api.search.tinyfish.ai`.
- Authentication is read only from `TINYFISH_API_KEY` and sent only as `X-API-Key`; never serialize, trace, persist, or place the value in model context.
- `provider = "model"` remains the default for backward compatibility.
- TinyFish is exposed only when the resolved web-search mode is `live`.
- No conversation history, source content, or inferred purpose is sent to TinyFish.
- Use `HttpClientFactory` and `ClientRouteClass::Other`, with HTTP request logging disabled.
- Preserve the existing namespaced `web.run` tool, web-search events, persistence, parallel-call support, and external-context marking.
- Tests must not mutate process environment.
- Support Linux, macOS, and Windows.
- Follow repository TDD rules: write each behavior test first, observe its expected failure, then add minimal production code.
- Run `just write-config-schema` after configuration shape changes, `just bazel-lock-update` after dependency changes, and `just fmt` after all code changes.

---

### Task 1: Web-search provider configuration

**Files:**
- Modify: `codex-rs/protocol/src/config_types.rs`
- Modify: `codex-rs/core/src/config/config_tests.rs`
- Generate: `codex-rs/core/config.schema.json`

**Interfaces:**
- Produces: `WebSearchProvider::{Model, Tinyfish}` with snake-case serde values and `Model` as the default.
- Produces: `WebSearchToolConfig.provider: Option<WebSearchProvider>` for layered TOML merging and resolved `WebSearchConfig.provider: WebSearchProvider`.
- Consumes: Existing `WebSearchToolConfig -> WebSearchConfig` conversion and merge behavior.

- [ ] **Step 1: Write failing config tests**

Add imports for `WebSearchProvider` and tests in `codex-rs/core/src/config/config_tests.rs` that parse the real TOML surface and load effective runtime config:

```rust
#[tokio::test]
async fn tools_web_search_tinyfish_provider_is_loaded() -> std::io::Result<()> {
    let codex_home = tempdir()?;
    let config_toml: ConfigToml = toml::from_str(
        r#"
web_search = "live"

[tools.web_search]
provider = "tinyfish"
"#,
    )
    .expect("TinyFish web-search config should deserialize");

    let config = Config::load_from_base_config_with_overrides(
        config_toml,
        ConfigOverrides::default(),
        codex_home.abs(),
    )
    .await?;

    assert_eq!(
        config.web_search_config.map(|config| config.provider),
        Some(WebSearchProvider::Tinyfish)
    );
    Ok(())
}

#[tokio::test]
async fn tools_web_search_provider_defaults_to_model() -> std::io::Result<()> {
    let codex_home = tempdir()?;
    let config_toml: ConfigToml = toml::from_str(
        r#"
[tools.web_search]
allowed_domains = ["docs.rs"]
"#,
    )
    .expect("web-search config should deserialize");

    let config = Config::load_from_base_config_with_overrides(
        config_toml,
        ConfigOverrides::default(),
        codex_home.abs(),
    )
    .await?;

    assert_eq!(
        config.web_search_config.map(|config| config.provider),
        Some(WebSearchProvider::Model)
    );
    Ok(())
}
```

Production change that makes these fail: removing provider deserialization/default propagation or mapping `tinyfish` to the wrong runtime variant.

- [ ] **Step 2: Run the focused tests and confirm RED**

Run:

```bash
just test -p codex-core tools_web_search_provider
```

Expected: compilation/test failure because `WebSearchProvider` and the provider fields do not exist.

- [ ] **Step 3: Add the minimal provider types and propagation**

In `codex-rs/protocol/src/config_types.rs`, add:

```rust
#[derive(
    Debug, Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq, Display, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum WebSearchProvider {
    #[default]
    Model,
    Tinyfish,
}
```

Add `pub provider: Option<WebSearchProvider>` to `WebSearchToolConfig` and `pub provider: WebSearchProvider` to `WebSearchConfig`. In `WebSearchToolConfig::merge`, use `other.provider.or(self.provider)`, allowing an explicitly selected `model` provider to override a lower `tinyfish` layer. In `From<WebSearchToolConfig> for WebSearchConfig`, resolve `config.provider.unwrap_or_default()`.

- [ ] **Step 4: Run the focused tests and confirm GREEN**

Run:

```bash
just test -p codex-core tools_web_search_provider
```

Expected: both new tests pass.

- [ ] **Step 5: Generate and inspect the config schema**

Run:

```bash
just write-config-schema
```

Confirm `codex-rs/core/config.schema.json` accepts only `model` and `tinyfish` for `tools.web_search.provider` and defaults to `model`.

- [ ] **Step 6: Commit Task 1**

```bash
git add codex-rs/protocol/src/config_types.rs codex-rs/core/src/config/config_tests.rs codex-rs/core/config.schema.json
git commit -m "feat(web): configure web search providers"
```

---

### Task 2: TinyFish REST client and normalized results

**Files:**
- Create: `codex-rs/ext/web-search/src/tinyfish.rs`
- Create: `codex-rs/ext/web-search/src/tinyfish_tests.rs`
- Modify: `codex-rs/ext/web-search/src/lib.rs`
- Modify: `codex-rs/ext/web-search/Cargo.toml`
- Modify: `codex-rs/Cargo.lock`
- Modify: `MODULE.bazel.lock`

**Interfaces:**
- Produces: `TINYFISH_SEARCH_ENDPOINT: &str` and `TINYFISH_API_KEY_ENV: &str`.
- Produces: `TinyFishSearchClient::new(HttpClientFactory, Url, RedactedString) -> Result<Self, TinyFishError>`.
- Produces: `TinyFishSearchClient::search(&TinyFishSearchRequest) -> Result<TinyFishSearchResponse, TinyFishError>`.
- Produces: `TinyFishSearchRequest { query, domains, recency_days, location }` and normalized serializable `TinyFishSearchResponse` / `TinyFishSearchResult` DTOs.

- [ ] **Step 1: Add test-only dependencies and a failing wire test**

Add `codex-http-client`, `codex-utils-redacted-string`, `serde`, `thiserror`, and Tokio time support to normal dependencies. Add Tokio macros/runtime and `wiremock` to dev-dependencies. Register a private `tinyfish` module from `lib.rs` with a sibling test module:

```rust
mod tinyfish;

#[cfg(test)]
#[path = "tinyfish_tests.rs"]
mod tinyfish_tests;
```

Write a wiremock test using `HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault)` and a mock endpoint. It must expect:

```text
GET /
X-API-Key: test-tinyfish-key
query=rust async traits
include_domains=doc.rust-lang.org,docs.rs
recency_minutes=43200
location=US
```

Return a literal TinyFish response with two results and assert equality against a hand-authored normalized `TinyFishSearchResponse` object.

Production change that makes this fail: wrong method, endpoint, auth header, domain join, days-to-minutes conversion, location mapping, response field mapping, or result order.

- [ ] **Step 2: Run the client test and confirm RED**

Run:

```bash
just test -p codex-web-search-extension tinyfish_search_sends_documented_request
```

Expected: compilation failure because the TinyFish client types do not exist.

- [ ] **Step 3: Implement the minimal client**

Implement these core types in `tinyfish.rs`:

```rust
pub(crate) const TINYFISH_SEARCH_ENDPOINT: &str = "https://api.search.tinyfish.ai";
pub(crate) const TINYFISH_API_KEY_ENV: &str = "TINYFISH_API_KEY";
const SEARCH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_ERROR_BODY_BYTES: usize = 1024;

#[derive(Clone)]
pub(crate) struct TinyFishSearchClient {
    client: HttpClient,
    endpoint: Url,
    api_key: RedactedString,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TinyFishSearchRequest {
    pub query: String,
    pub domains: Option<Vec<String>>,
    pub recency_days: Option<u64>,
    pub location: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct TinyFishSearchResponse {
    pub query: String,
    pub results: Vec<TinyFishSearchResult>,
    pub total_results: u64,
    #[serde(default)]
    pub page: u64,
}
```

Use private wire DTOs where optional TinyFish fields need `#[serde(default)]`. Build the policy-aware client with request logging disabled and send the API key only through `.header("X-API-Key", api_key.as_str())`.

- [ ] **Step 4: Run the wire test and confirm GREEN**

Run the same focused test and expect it to pass.

- [ ] **Step 5: Add failing safe-error tests**

Add separate tests for:

- `401` maps to the exact safe message `TinyFish rejected TINYFISH_API_KEY`.
- `429` maps to the exact safe message `TinyFish web search rate limit exceeded`.
- A `500` body longer than 1 KiB is truncated and neither the body nor the formatted error contains `test-tinyfish-key`.
- `u64::MAX` recency returns an overflow error before wiremock receives a request.

Production changes these catch: leaking credentials, returning unbounded provider bodies, misclassifying actionable statuses, or wrapping recency arithmetic.

- [ ] **Step 6: Run the safe-error tests and confirm RED**

Run:

```bash
just test -p codex-web-search-extension tinyfish_error
```

Expected: failures until status and overflow mapping are implemented.

- [ ] **Step 7: Implement bounded status and overflow handling**

Add a `TinyFishError` enum with distinct configuration, request, HTTP-status, response-decode, and recency-overflow cases. Read at most `MAX_ERROR_BODY_BYTES` when formatting non-success bodies. Use `checked_mul(24 * 60)` for recency conversion.

- [ ] **Step 8: Run all TinyFish client tests and confirm GREEN**

```bash
just test -p codex-web-search-extension tinyfish
```

- [ ] **Step 9: Refresh dependency locks**

```bash
just bazel-lock-update
```

Confirm `Cargo.lock` and `MODULE.bazel.lock` contain only dependency-edge changes required by `codex-web-search-extension`.

- [ ] **Step 10: Commit Task 2**

```bash
git add codex-rs/ext/web-search codex-rs/Cargo.lock MODULE.bazel.lock
git commit -m "feat(web): add TinyFish search client"
```

---

### Task 3: Route `web.run` through TinyFish

**Files:**
- Create: `codex-rs/ext/web-search/tinyfish_web_run_description.md`
- Modify: `codex-rs/ext/web-search/BUILD.bazel`
- Modify: `codex-rs/ext/web-search/src/extension.rs`
- Modify: `codex-rs/ext/web-search/src/schema.rs`
- Modify: `codex-rs/ext/web-search/src/tool.rs`
- Modify: `codex-rs/core/tests/suite/responses_lite.rs`

**Interfaces:**
- Consumes: `WebSearchConfig.provider` from Task 1.
- Consumes: `TinyFishSearchClient` and normalized DTOs from Task 2.
- Produces: Provider-specific `WebSearchBackend::{Model, Tinyfish}` inside the extension.
- Produces: TinyFish search-only `web.run` schema and normalized JSON function output.

- [ ] **Step 1: Write failing extension provider tests**

Extend the existing extension tests to prove:

1. `provider = tinyfish` plus `WebSearchMode::Live` contributes exactly `ToolName::namespaced("web", "run")` even when the model provider does not support standalone search.
2. The same TinyFish configuration contributes no tool in `Disabled`, `Cached`, or `Indexed` modes.
3. The TinyFish tool schema contains `search_query` and `response_length`, requires `search_query`, caps it at four entries, and omits `open`, `click`, `find`, `screenshot`, `image_query`, `finance`, `weather`, `sports`, and `time`.

Production changes these catch: incorrect permission gating, accidental dependence on Qwen model-provider metadata, duplicate tool names, or exposing unsupported commands.

- [ ] **Step 2: Run the extension tests and confirm RED**

```bash
just test -p codex-web-search-extension tinyfish_provider
```

Expected: failures because provider-specific availability and schema selection do not exist.

- [ ] **Step 3: Add provider-specific extension state and schema**

Add an internal backend enum. Preserve the current model-provider fields for `Model`; for `Tinyfish`, carry `HttpClientFactory`, the fixed parsed endpoint, and an optional redacted key read from `TINYFISH_API_KEY` when `WebSearchExtensionConfig` is created.

Add a `TinyFishCommands` schema with this shape:

```rust
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct TinyFishCommands {
    #[schemars(length(min = 1, max = 4))]
    pub search_query: Vec<SearchQuery>,
    pub response_length: Option<SearchResponseLength>,
}
```

Add a TinyFish-specific description that tells the model the tool searches the public web, accepts one to four queries, and returns untrusted results. Do not mention unsupported commands.

- [ ] **Step 4: Run the extension tests and confirm GREEN**

Run the same focused test command and expect all provider/schema tests to pass.

- [ ] **Step 5: Write failing tool execution tests**

Add tests around pure request preparation and response formatting that prove:

- An empty query is rejected.
- Five queries are rejected.
- Per-query domains cannot widen configured allowed domains; matching is case-insensitive and the emitted order follows the configured allowlist.
- An empty intersection is rejected without an HTTP call.
- `short` returns five results while `medium`, `long`, and omission return up to ten.
- Multiple query responses stay grouped by executed query and preserve ranking.
- The API key and conversation-history fixture text do not appear in the outbound query or model-visible output.

Production changes these catch: unsafe filter widening, query fanout beyond the contract, privacy regression, result interleaving, or response-length regression.

- [ ] **Step 6: Run execution tests and confirm RED**

```bash
just test -p codex-web-search-extension tinyfish_tool
```

- [ ] **Step 7: Implement TinyFish execution behind the existing tool**

Refactor `WebSearchTool::handle_call` into provider-specific private methods. The TinyFish path must:

1. Parse `TinyFishCommands`.
2. Validate one to four non-empty queries.
3. Resolve effective domains by case-insensitive intersection with configured domains.
4. Emit the existing started event.
5. Execute searches in input order.
6. Truncate per-query results according to `response_length`.
7. Serialize this exact top-level shape as pretty JSON:

```json
{
  "provider": "tinyfish",
  "searches": [
    {
      "query": "rust async traits",
      "results": []
    }
  ]
}
```

8. Flatten normalized result objects into the existing completion event's opaque `results` array.
9. Return through `SearchOutput`, retaining `contains_external_context() == true`.

Map missing `TINYFISH_API_KEY` and argument failures to `FunctionCallError::RespondToModel`; do not abort the thread.

- [ ] **Step 8: Run TinyFish tool tests and confirm GREEN**

```bash
just test -p codex-web-search-extension tinyfish_tool
```

- [ ] **Step 9: Write and run the core integration test**

In `responses_lite.rs`, configure a custom/Qwen-style provider with `supports_standalone_web_search = false`, set live mode, select `WebSearchProvider::Tinyfish`, install the web-search extension, and submit one turn. Assert on the real Responses request:

```rust
assert!(request.tool_by_name("web", "run").is_some());
assert!(!has_hosted_tool(tools, "web_search"));
```

Run first before production wiring is complete and confirm it fails, then run after wiring and confirm it passes:

```bash
just test -p codex-core responses_lite_exposes_tinyfish_web_search
```

- [ ] **Step 10: Run required focused verification**

```bash
just test -p codex-web-search-extension
just test -p codex-protocol
just test -p codex-core responses_lite_exposes_tinyfish_web_search
```

- [ ] **Step 11: Run lint fixes and formatting**

```bash
just fix -p codex-web-search-extension
just fmt
```

Per repository instructions, do not rerun tests after `fix` or `fmt`.

- [ ] **Step 12: Commit Task 3**

```bash
git add codex-rs/ext/web-search codex-rs/core/tests/suite/responses_lite.rs
git commit -m "feat(web): route standalone search through TinyFish"
```

---

### Task 4: Installation smoke check and operator handoff

**Files:**
- Modify only if required by observed behavior: `README.md`

**Interfaces:**
- Consumes: Completed TinyFish provider.
- Produces: A locally built ZIPCODE binary that exposes TinyFish `web.run` when configured.

- [ ] **Step 1: Build the ZIPCODE CLI**

```bash
cargo build -p zipcode-cli --release
```

Expected binary: `codex-rs/target/release/zip-code` or the package-defined equivalent discovered from `codex-rs/zipcode-cli/Cargo.toml`.

- [ ] **Step 2: Validate configuration without exposing credentials**

Launch the built binary with a temporary `CODEX_HOME` containing:

```toml
web_search = "live"

[tools.web_search]
provider = "tinyfish"
```

With no key, invoke `web.run` through the existing test harness and confirm the model receives the missing-`TINYFISH_API_KEY` error rather than a crash. With a user-supplied environment key, run one query and confirm ranked TinyFish results return.

- [ ] **Step 3: Document only setup that is not self-evident**

If the CLI has no existing configuration reference that can express the two required settings, add a concise root README section containing only:

```bash
export TINYFISH_API_KEY="..."
```

and the TOML block above. Do not place the key itself in any file.

- [ ] **Step 4: Commit any necessary handoff documentation**

```bash
git add README.md
git commit -m "docs: configure TinyFish web search"
```

Skip this commit when no documentation change is required.
