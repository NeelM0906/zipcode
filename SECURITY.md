# Security

## Pilot limitations

The current deployment uses one shared pilot bearer at the HTTPS edge. A leaked
credential requires team-wide rotation, and requests are not attributable to
individual users. Before broader deployment, replace it with per-user or
per-service short-lived credentials, revocation, rate limits, quotas, request
accounting, and auditable identities that do not log prompts or source code.

## Never commit

- ZIPCODE, ngrok, GitHub, model-provider, or Hugging Face credentials.
- Rendered traffic policies containing a bearer value.
- `~/.zipcode`, Codex session state, SQLite databases, or shell history.
- Model weights, caches, raw prompts, tool arguments, tool output, or repository
  contents captured from agent sessions.

The committed gateway is designed to log request IDs and tool structure rather
than prompts, tool arguments, tool results, or authorization headers.

## Reporting

Report suspected credential exposure privately to the repository owner. Do not
open a public issue containing a credential, private endpoint policy, prompt,
or source-bearing request log.
