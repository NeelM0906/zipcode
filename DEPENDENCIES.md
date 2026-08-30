# Pinned dependencies

The deployment records intentionally reference immutable model and runtime
identities.

| Component | Identity |
|---|---|
| Qwen3.8-27B FP8 | `Qwen/Qwen3.8-27B-FP8` revision `017b9c7af6b5689d5dd426a76e0bc077eb5ca20a` |
| Flash-Next NVFP4 | `RadixArk/Qwen3.8-Flash-Next-NVFP4` revision `7b719225242aacd3dbd3f9407468c2ee9a9d2594` |
| SGLang image | `lmsysorg/sglang@sha256:616a3e97f45191af975896cfa644279096cb31bd408a071c2e99ca7209c3cafe` |
| RTX PRO 6000 SGLang fork | `jpezzulli/sglang-rtxpro6000` commit `16e5682aad6e3335f38ec8d5711176278f12a086` |
| Executable SGLang source baseline | `64ecd64924fee338e3bf846a32167cd604186827` |
| ZIPCODE/Codex harness | Codex CLI `0.150.1` |

The local SGLang checkout was clean at the pinned commit when this repository
was assembled; no uncommitted runtime patch was omitted. Model weights and the
SGLang checkout are external dependencies and are not duplicated here.
