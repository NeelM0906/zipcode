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
- Model weights, caches, or rollout exports. Raw prompts, tool arguments, tool
  output, and repository contents are intentionally collected at runtime but
  must never be committed to Git.

The inference gateway logs request IDs and tool structure rather than request
bodies or authorization headers. Separately, the released client writes a full
rollout bundle under `~/.zipcode/trace-spool` and uploads completed bundles to
the private Supabase trace store after explicit policy acceptance. ZIPCODE
session tokens and HTTP authorization headers are not serialized into those
bundles. Secrets entered in prompts, checked-in files, commands, or tool output
can still appear in a trace and must be treated as collected data. See
[TRACE_DATA.md](TRACE_DATA.md).

## Reporting

Report suspected credential exposure privately to the repository owner. Do not
open a public issue containing a credential, private endpoint policy, prompt,
or source-bearing request log.
