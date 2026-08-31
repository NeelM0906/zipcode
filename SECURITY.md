# Security

## Authentication boundary

ZIPCODE uses GitHub device authorization to establish identity. The service
accepts only invited GitHub logins, issues 15-minute signed access tokens, and
rotates opaque refresh tokens. Revoking an invitation immediately blocks new
requests and invalidates refresh sessions. GitHub access tokens are checked
during exchange and are never stored by ZIPCODE.

The client keeps its ZIPCODE session in the operating-system credential store
when available, with a mode-0600 `~/.zipcode/auth.json` fallback. The gateway
stores only SHA-256 hashes of refresh tokens. Set `ZIPCODE_JWT_SECRET` to at
least 32 random bytes and keep the SQLite authentication database outside the
repository.

Rate limits and quotas are deployment concerns and should be enabled at the
edge before admitting untrusted users. Authentication does not make a shared
model service a hard multi-tenant isolation boundary.

## Never commit

- ZIPCODE, ngrok, GitHub, model-provider, or Hugging Face credentials.
- JWT signing secrets or rendered traffic policies containing secrets.
- `~/.zipcode`, Codex session state, SQLite databases, or shell history.
- Model weights, caches, raw prompts, tool arguments, tool output, or repository
  contents captured from agent sessions.

The committed gateway is designed to log request IDs and tool structure rather
than prompts, tool arguments, tool results, or authorization headers.

## Reporting

Report suspected credential exposure privately to the repository owner. Do not
open a public issue containing a credential, private endpoint policy, prompt,
or source-bearing request log.
