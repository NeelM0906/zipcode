# Qwen3.8 Flash-Next canary

This directory isolates the Flash-Next checkpoint, the pinned RTX PRO 6000
SGLang fork, compiler caches, and the GPU 1 canary service from the production
Qwen3.8-27B service.

Pinned identities:

- model: `RadixArk/Qwen3.8-Flash-Next-NVFP4` at
  `7b719225242aacd3dbd3f9407468c2ee9a9d2594`
- fork branch head: `16e5682aad6e3335f38ec8d5711176278f12a086`
- executable source: `64ecd64924fee338e3bf846a32167cd604186827`
- initial context gate: 524,288 tokens with factor-2 YaRN

The canary uses GPU UUID `GPU-472867f8-ccad-ec82-d962-5adbbbec83fb`, port
8020 for SGLang, port 8022 for its Codex Responses compatibility gateway, and
port 8002 for the public two-model mux.
`activate-gpu1-canary.sh` drains only the existing GPU 1 replica and restores it
automatically if the canary cannot become healthy within 30 minutes.

## Configure

Copy `.env.example` to `.env`, then set the model, source, cache, and GPU UUID
paths for this host. Create a private authentication directory owned by the
UID/GID configured in `.env`:

```bash
install -d -m 700 /absolute/path/to/zipcode-auth-data
openssl rand -hex 32
```

Put the generated value in `ZIPCODE_JWT_SECRET` and set
`ZIPCODE_AUTH_DATA_PATH` to the private directory. Keep `.env` out of version
control. The dedicated gateway image is built from `gateway/Dockerfile`; only
the model service uses the pinned SGLang image.

## Start and invite

```bash
docker compose build codex-gateway model-mux
docker compose up -d
docker compose exec model-mux python /mux/invitations.py invite NeelM0906
docker compose ps
curl --fail http://127.0.0.1:8002/health
```

The origin binds only to loopback. Put an HTTPS edge in front of port 8002 and
let the origin enforce the short-lived ZIPCODE bearer tokens. Do not add a
second fixed bearer credential at the edge; it would block device-login and
token-refresh endpoints. The example in `deploy/ngrok` passes traffic through
for origin authentication.

Manage access with the same container command:

```bash
docker compose exec model-mux python /mux/invitations.py list
docker compose exec model-mux python /mux/invitations.py revoke GITHUB_LOGIN
```
