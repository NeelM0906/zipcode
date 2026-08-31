# Authentication

ZIPCODE authentication is separate from OpenAI and ChatGPT accounts.

## Bootstrap the GitHub OAuth app

The repository owner creates a GitHub OAuth App under **Settings > Developer
settings > OAuth Apps** with these values:

- Application name: `ZIPCODE CLI`
- Homepage URL: `https://github.com/NeelM0906/zipcode`
- Authorization callback URL: `http://127.0.0.1`
- Device Flow: enabled

Only the public client ID is embedded in release builds. Store it as the
repository variable `ZIPCODE_GITHUB_CLIENT_ID`; ZIPCODE does not require or
distribute the OAuth client secret.

Run:

```bash
zip-code login
```

The CLI opens GitHub's device-authorization page and shows a one-time code.
After GitHub confirms your identity, the ZIPCODE service checks that your
GitHub login has an active invitation and returns a short-lived service
session. Check or remove the session with:

```bash
zip-code login status
zip-code logout
```

Access tokens last 15 minutes by default. The client rotates its refresh token
automatically, and the Codex provider hook retries with a fresh access token
after a 401 response.

## Manage invitations

On the gateway host, point `ZIPCODE_AUTH_DATABASE` at the mounted database and
run:

```bash
python3 gateway/invitations.py invite GITHUB_LOGIN
python3 gateway/invitations.py invite GITHUB_LOGIN --days 30
python3 gateway/invitations.py list
python3 gateway/invitations.py revoke GITHUB_LOGIN
```

Revocation blocks access immediately. The optional
`ZIPCODE_GITHUB_ALLOWLIST` environment variable is intended only to bootstrap
the first operator account.

## Server secrets

Generate a signing secret with `openssl rand -hex 32`, store it as
`ZIPCODE_JWT_SECRET`, and back up the authentication database as sensitive
operator data. Never commit either value. See [SECURITY.md](../SECURITY.md) for
the full boundary.
