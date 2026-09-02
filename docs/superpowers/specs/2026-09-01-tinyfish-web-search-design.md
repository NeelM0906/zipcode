# TinyFish Web Search Design

## Objective

Give ZIPCODE's locally hosted Qwen models a first-class web search capability by routing the existing standalone `web.run` tool through TinyFish Search. The implementation must not route model prompts, conversation history, source code, or model inference through TinyFish.

## Phase 1 scope

Phase 1 supports web search queries only. It intentionally does not implement TinyFish Fetch, page opening, link clicking, PDF screenshots, image search, finance, weather, sports, or time lookup. Those operations remain available only through the legacy model-provider search backend.

The model-facing tool remains the existing namespaced `web.run` tool. Keeping that name preserves the current tool-selection, event, persistence, and TUI behavior, and prevents the hosted `web_search` tool from being exposed alongside the standalone implementation.

## Configuration

Extend `[tools.web_search]` with an explicit provider:

```toml
web_search = "live"

[tools.web_search]
provider = "tinyfish"
```

Supported values are:

- `model`: the existing behavior, which calls the selected model provider's `/search` endpoint.
- `tinyfish`: a direct call to the TinyFish Search REST API.

`model` remains the default when `provider` is omitted so existing Codex and custom-provider configurations do not change behavior.

TinyFish authentication comes exclusively from the `TINYFISH_API_KEY` environment variable. ZIPCODE must never accept the key in `config.toml`, command arguments, tool arguments, traces, or model-visible context. The key is sent only in the `X-API-Key` header to the fixed endpoint `https://api.search.tinyfish.ai`.

TinyFish is a live external search provider. When `provider = "tinyfish"`, the standalone tool is exposed only when the resolved `web_search` mode is `live`. Cached and indexed modes must not silently perform live TinyFish requests.

## Architecture

The implementation stays inside `codex-rs/ext/web-search`, which already owns the standalone web-search tool.

### Provider selection

`WebSearchExtensionConfig` records the selected web-search provider. The extension contributes one `web.run` executor:

- The `model` executor preserves the existing `SearchClient` request path and full command schema.
- The `tinyfish` executor uses a focused TinyFish client and a search-only schema.

The two providers share the existing `WebSearchBegin` and `WebSearchEnd` events, `WebSearchItem` persistence, external-context marking, parallel-call support, and output transport.

### TinyFish client

Add a focused `tinyfish.rs` module with:

- request and response DTOs matching the documented TinyFish Search REST contract;
- a client built through `HttpClientFactory` for the fixed destination and `ClientRouteClass::Other`;
- request logging disabled so credential-bearing request diagnostics cannot expose headers;
- a ten-second request timeout;
- explicit status handling for authentication, access, rate-limit, and upstream failures;
- response parsing into ZIPCODE-owned normalized result DTOs.

The client accepts an endpoint and redacted API key through its constructor so wire-level tests can use a local mock server without modifying process environment. Production construction supplies the fixed endpoint and reads `TINYFISH_API_KEY` once when the tool is created.

### Model-facing input

The TinyFish `web.run` schema exposes:

```json
{
  "search_query": [
    {
      "q": "Rust async trait official documentation",
      "domains": ["doc.rust-lang.org"],
      "recency": 30
    }
  ],
  "response_length": "short"
}
```

Rules:

- `search_query` is required and accepts one to four queries.
- `q` must be non-empty.
- `domains` is optional and maps to TinyFish `include_domains`.
- `recency` is optional, remains expressed in days to preserve the existing ZIPCODE contract, and maps to TinyFish `recency_minutes` using checked multiplication.
- `response_length` is optional. `short` returns at most five results per query; `medium` and `long` return at most ten because TinyFish currently returns ten ranked results per page.
- The configured country maps to TinyFish `location`.
- No conversation history or inferred search purpose is included in the TinyFish request.

When both a global allowed-domain list and a per-query domain list exist, the effective list is their case-insensitive intersection. An empty intersection is rejected before making a network request. This prevents a tool call from widening the configured restriction.

### Model-facing output

Return normalized JSON text containing the provider, executed query, and ranked results. Each result may contain:

- `position`
- `site_name`
- `title`
- `snippet`
- `url`
- `date`
- `publisher`
- academic metadata when TinyFish returns it

The same normalized result objects are emitted through the existing web-search completion event. Result text remains marked as external context so the harness treats it as untrusted web content.

### Errors

Errors must be actionable but must not expose the API key or unbounded response bodies:

- Missing key: tell the model that `TINYFISH_API_KEY` must be configured.
- `401`: report that the configured TinyFish key was rejected.
- `402` or `403`: report that the account lacks Search API access.
- `429`: report the TinyFish rate limit.
- Other non-success responses: include the status and at most 1 KiB of response text.
- Invalid arguments, including zero queries, more than four queries, empty query text, impossible domain intersections, and recency overflow, are model-correctable errors and do not terminate the session.

## Security and privacy invariants

- The API key is never serialized or formatted with a plaintext `Debug` implementation.
- The API key cannot be supplied by project-local tool input.
- The endpoint is fixed in production, preventing a malicious repository configuration from redirecting the credential.
- Only explicit query text, domain filters, recency, and approximate country leave the machine.
- TinyFish output is untrusted external context and is bounded by the existing tool-output truncation policy.
- TinyFish search is never performed when the resolved web-search permission is cached, indexed, or disabled.

## Verification

The implementation requires:

1. Config tests proving `provider = "tinyfish"` parses and omission preserves `model`.
2. Wire-level TinyFish client tests proving the exact method, query parameters, `X-API-Key` header, response normalization, safe error mapping, and domain restriction behavior.
3. Extension tests proving provider-specific tool schema selection and live-only availability.
4. A core integration test proving the Qwen/custom-provider request receives the standalone `web.run` tool when TinyFish is configured, without a hosted `web_search` tool.
5. `just write-config-schema`, `just fmt`, `just test -p codex-web-search-extension`, the affected config/protocol tests, and the focused core integration test.

## Follow-up phase

TinyFish Fetch will be added as a separate provider capability after search ships. It will support URL opening and extracted Markdown while retaining the same credential, outbound-routing, untrusted-context, size-limit, and permission invariants. Fetch is not part of this change.
