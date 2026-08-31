-- ZIPCODE stores large, full-fidelity rollout bundles in a private Supabase
-- Storage bucket. Postgres contains only the searchable index, integrity
-- metadata, consent acknowledgements, feedback, and dataset export manifests.

create table public.zipcode_trace_consents (
    github_login text primary key,
    policy_version integer not null check (policy_version > 0),
    accepted_at timestamptz not null,
    last_seen_at timestamptz not null default now(),
    revoked_at timestamptz,
    metadata jsonb not null default '{}'::jsonb
        check (jsonb_typeof(metadata) = 'object'),
    check (github_login = lower(github_login)),
    check (char_length(github_login) between 1 and 39),
    check (revoked_at is null or revoked_at >= accepted_at)
);

create table public.zipcode_trace_sessions (
    trace_id uuid primary key,
    rollout_id text not null,
    root_thread_id text not null,
    github_login text not null
        references public.zipcode_trace_consents(github_login),
    schema_version integer not null check (schema_version > 0),
    capture_policy_version integer not null check (capture_policy_version > 0),
    client_version text not null,
    status text not null default 'uploading'
        check (status in ('uploading', 'complete', 'failed')),
    started_at timestamptz not null,
    ended_at timestamptz,
    upload_started_at timestamptz not null default now(),
    completed_at timestamptz,
    bundle_sha256 text not null check (bundle_sha256 ~ '^[0-9a-f]{64}$'),
    total_bytes bigint not null check (total_bytes >= 0),
    part_count integer not null check (part_count > 0),
    storage_prefix text not null unique,
    model text,
    repository_path text,
    repository_remote text,
    repository_commit text,
    metadata jsonb not null default '{}'::jsonb
        check (jsonb_typeof(metadata) = 'object'),
    check (ended_at is null or ended_at >= started_at),
    check (
        (status = 'complete' and completed_at is not null)
        or (status <> 'complete' and completed_at is null)
    )
);

create table public.zipcode_trace_parts (
    trace_id uuid not null
        references public.zipcode_trace_sessions(trace_id) on delete cascade,
    part_number integer not null check (part_number >= 0),
    object_path text not null unique,
    size_bytes bigint not null check (size_bytes > 0),
    sha256 text not null check (sha256 ~ '^[0-9a-f]{64}$'),
    uploaded_at timestamptz not null default now(),
    primary key (trace_id, part_number)
);

create table public.zipcode_trace_feedback (
    id bigint generated always as identity primary key,
    trace_id uuid not null
        references public.zipcode_trace_sessions(trace_id) on delete cascade,
    github_login text not null,
    turn_id text,
    source text not null check (source in ('user', 'verifier', 'ci', 'curator')),
    label text not null,
    score double precision,
    comment text,
    metadata jsonb not null default '{}'::jsonb
        check (jsonb_typeof(metadata) = 'object'),
    created_at timestamptz not null default now()
);

create table public.zipcode_trace_dataset_exports (
    id uuid primary key default gen_random_uuid(),
    name text not null,
    version text not null,
    format text not null check (format in ('jsonl', 'parquet')),
    object_path text not null unique,
    sha256 text not null check (sha256 ~ '^[0-9a-f]{64}$'),
    row_count bigint not null check (row_count >= 0),
    source_filter jsonb not null default '{}'::jsonb
        check (jsonb_typeof(source_filter) = 'object'),
    created_by text not null,
    created_at timestamptz not null default now(),
    unique (name, version)
);

create index zipcode_trace_sessions_actor_started_idx
    on public.zipcode_trace_sessions (github_login, started_at desc);
create index zipcode_trace_sessions_uploading_idx
    on public.zipcode_trace_sessions (upload_started_at)
    where status = 'uploading';
create index zipcode_trace_feedback_trace_created_idx
    on public.zipcode_trace_feedback (trace_id, created_at desc);
create index zipcode_trace_feedback_label_created_idx
    on public.zipcode_trace_feedback (label, created_at desc);

alter table public.zipcode_trace_consents enable row level security;
alter table public.zipcode_trace_sessions enable row level security;
alter table public.zipcode_trace_parts enable row level security;
alter table public.zipcode_trace_feedback enable row level security;
alter table public.zipcode_trace_dataset_exports enable row level security;

alter table public.zipcode_trace_consents force row level security;
alter table public.zipcode_trace_sessions force row level security;
alter table public.zipcode_trace_parts force row level security;
alter table public.zipcode_trace_feedback force row level security;
alter table public.zipcode_trace_dataset_exports force row level security;

revoke all on public.zipcode_trace_consents from public, anon, authenticated;
revoke all on public.zipcode_trace_sessions from public, anon, authenticated;
revoke all on public.zipcode_trace_parts from public, anon, authenticated;
revoke all on public.zipcode_trace_feedback from public, anon, authenticated;
revoke all on public.zipcode_trace_dataset_exports from public, anon, authenticated;
revoke all on sequence public.zipcode_trace_feedback_id_seq
    from public, anon, authenticated;

grant select, insert, update, delete
    on public.zipcode_trace_consents,
       public.zipcode_trace_sessions,
       public.zipcode_trace_parts,
       public.zipcode_trace_feedback,
       public.zipcode_trace_dataset_exports
    to service_role;
grant usage, select on sequence public.zipcode_trace_feedback_id_seq
    to service_role;

comment on table public.zipcode_trace_sessions is
    'Index of full-fidelity ZIPCODE rollout bundles stored in private Storage.';
comment on table public.zipcode_trace_parts is
    'Integrity metadata for fixed-size bundle parts in the zipcode-rollout-traces bucket.';
