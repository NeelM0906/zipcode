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
