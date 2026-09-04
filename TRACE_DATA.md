# Trace collection and storage

ZIPCODE collects completed coding-agent rollouts for evaluation, reward-model
work, and fine-tuning dataset preparation. Collection is enabled for this team
build, but it does not begin until a user accepts the displayed policy.
Declining exits without starting the coding runtime. A policy-version change
requires a new acceptance. Set `ZIPCODE_DISABLE_TRACE_UPLOAD=1` before launch
to disable both local trace capture and remote upload. The launcher also
removes any inherited trace-root setting from the coding runtime in this mode.

## What is collected

The rollout bundle is full fidelity and can contain:

- prompts, model responses, and reasoning items emitted by the model;
- tool and MCP calls, arguments, results, terminal commands, and terminal output;
- source-code context, file paths, patches, repository remote, and commit;
- compaction checkpoints, child-agent exchanges, timing, token usage, and errors.

Model-internal state that is never emitted by the serving API is not available
to the client and cannot be collected. ZIPCODE authentication tokens and HTTP
authorization headers are not added to rollout bundles. This is not a general
secret scrubber: a secret typed into a prompt or exposed through a file,
command, or tool result can be present in the uploaded trace.

## Where it goes

During a session, raw events and payloads are written below
`~/.zipcode/trace-spool`. After the runtime exits, the launcher replays the
bundle into a reduced `state.json`, creates a `tar.gz`, and uploads it in
SHA-256-verified 4 MiB parts. Interrupted uploads retry on the next launch and
the local bundle is retained after successful upload.

Remote data is stored in Supabase project `qudfqzabhkrhbeuvvqmt`:

- private Storage bucket `zipcode-rollout-traces` holds the bundle parts;
- `zipcode_trace_sessions` and `zipcode_trace_parts` hold ownership, search,
  completion, and integrity metadata;
- `zipcode_trace_consents` records the accepted policy version and timestamp;
- `zipcode_trace_feedback` and `zipcode_trace_dataset_exports` are reserved for
  reviewed labels and reproducible dataset manifests.

The public and authenticated Supabase roles have no table grants. All trace
tables have forced row-level security with no end-user policies, so browser or
client-side access is denied. The ingest Edge Function uses the caller's
short-lived ZIPCODE token to resolve the invited GitHub login, then performs
the narrowly scoped upload with Supabase's server-side service role. The
service-role key is never shipped in the ZIPCODE client.

## Retention and access

There is no automatic retention job yet. Removing `~/.zipcode/trace-spool` only
removes local copies. The ingest service provides an authenticated,
owner-checked `DELETE /sessions/{trace_id}` operation that deletes Storage
objects through the Storage API before removing Postgres metadata; it is not
yet exposed as a CLI command. Operators must treat the Supabase project as
source-code-bearing sensitive infrastructure, restrict dashboard and
service-role access, and fulfill remote deletion requests. Do not use raw
traces directly for training: build a versioned export, run secret and PII
review, attach quality/license labels, and record its source filter and SHA-256
in `zipcode_trace_dataset_exports`.

For CI or another non-interactive environment, acceptance can be provided with
`ZIPCODE_ACCEPT_FULL_TRACE=1`. Setting it has the same meaning as typing
`I AGREE`; only use it after the person or organization responsible for that
environment has approved this policy. `ZIPCODE_DISABLE_TRACE_UPLOAD=1` takes
precedence over acceptance. Existing local bundles remain on disk and are not
uploaded while tracing is disabled.
