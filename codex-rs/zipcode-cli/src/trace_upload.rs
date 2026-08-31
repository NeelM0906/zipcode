use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use chrono::DateTime;
use chrono::SecondsFormat;
use chrono::Utc;
use codex_rollout_trace::REDUCED_STATE_FILE_NAME;
use codex_rollout_trace::RolloutStatus;
use codex_rollout_trace::replay_bundle;
use flate2::Compression;
use flate2::write::GzEncoder;
use reqwest::Client;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use std::fs::File;
use std::io::IsTerminal;
use std::io::Read;
use std::io::Write;
use std::path::Path;

const CAPTURE_POLICY_VERSION: u32 = 1;
const PART_BYTES: u64 = 4 * 1024 * 1024;
const DEFAULT_INGEST_URL: &str =
    "https://qudfqzabhkrhbeuvvqmt.supabase.co/functions/v1/zipcode-trace-ingest";
const CONSENT_FILE: &str = "full-trace-consent.json";
const UPLOADED_MARKER: &str = ".supabase-uploaded.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CaptureConsent {
    policy_version: u32,
    accepted_at: String,
}

#[derive(Debug, Serialize)]
struct CreateSessionRequest<'a> {
    trace_id: &'a str,
    rollout_id: &'a str,
    root_thread_id: &'a str,
    schema_version: u32,
    capture_policy_version: u32,
    consent_accepted_at: &'a str,
    client_version: &'static str,
    started_at: String,
    ended_at: Option<String>,
    bundle_sha256: &'a str,
    total_bytes: u64,
    part_count: u64,
    model: Option<&'a str>,
    repository_path: Option<String>,
    repository_remote: Option<String>,
    repository_commit: Option<String>,
    metadata: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct SessionResponse {
    status: String,
}

#[derive(Debug, Serialize)]
struct UploadedMarker<'a> {
    trace_id: &'a str,
    bundle_sha256: &'a str,
    uploaded_at: String,
    backend: &'static str,
}

pub(crate) fn ensure_capture_consent(home: &Path) -> Result<CaptureConsent> {
    let path = home.join(CONSENT_FILE);
    if let Ok(serialized) = std::fs::read_to_string(&path)
        && let Ok(consent) = serde_json::from_str::<CaptureConsent>(&serialized)
        && consent.policy_version == CAPTURE_POLICY_VERSION
    {
        return Ok(consent);
    }

    eprintln!(
        "\nZIPCODE FULL TRACE COLLECTION\n\
         ZIPCODE records and uploads every prompt, model response, emitted reasoning,\n\
         tool call and result, source-code context, patch, terminal command/output,\n\
         path, compaction, and sub-agent exchange for evaluation and model training.\n\
         ZIPCODE auth tokens and HTTP auth headers are not added to the trace. Secrets\n\
         entered in prompts, files, commands, or tool output can still be captured.\n\
         Use of ZIPCODE requires acceptance.\n"
    );
    let accepted = std::env::var("ZIPCODE_ACCEPT_FULL_TRACE").as_deref() == Ok("1");
    if !accepted {
        if !std::io::stdin().is_terminal() {
            bail!(
                "full trace collection requires interactive acceptance or ZIPCODE_ACCEPT_FULL_TRACE=1"
            );
        }
        eprint!("Type I AGREE to continue: ");
        std::io::stderr().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if answer.trim() != "I AGREE" {
            bail!("full trace collection was not accepted");
        }
    }

    let consent = CaptureConsent {
        policy_version: CAPTURE_POLICY_VERSION,
        accepted_at: now_rfc3339(),
    };
    std::fs::write(&path, serde_json::to_vec_pretty(&consent)?)?;
    super::set_private_permissions(&path)?;
    Ok(consent)
}

pub(crate) async fn sync_completed_traces(
    home: &Path,
    access_token: &str,
    consent: &CaptureConsent,
) -> Result<()> {
    let trace_root = home.join("trace-spool");
    if !trace_root.is_dir() {
        return Ok(());
    }
    let client = Client::builder()
        .user_agent(concat!("zipcode-cli/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(120))
        .build()?;
    let mut bundles = std::fs::read_dir(&trace_root)?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join("manifest.json").is_file())
        .collect::<Vec<_>>();
    bundles.sort();
    for bundle in bundles {
        if bundle.join(UPLOADED_MARKER).exists() {
            continue;
        }
        let trace = match replay_bundle(&bundle) {
            Ok(trace) if trace.status != RolloutStatus::Running => trace,
            Ok(_) => continue,
            Err(error) => {
                eprintln!(
                    "ZIPCODE: skipping unreadable trace bundle {}: {error:#}",
                    bundle.display()
                );
                continue;
            }
        };
        std::fs::write(
            bundle.join(REDUCED_STATE_FILE_NAME),
            serde_json::to_vec_pretty(&trace)?,
        )?;
        upload_bundle(&client, access_token, consent, &bundle, &trace).await?;
    }
    Ok(())
}

async fn upload_bundle(
    client: &Client,
    access_token: &str,
    consent: &CaptureConsent,
    bundle: &Path,
    trace: &codex_rollout_trace::RolloutTrace,
) -> Result<()> {
    let upload_dir = bundle
        .parent()
        .context("trace bundle has no parent directory")?
        .join(".upload-cache");
    std::fs::create_dir_all(&upload_dir)?;
    let archive = upload_dir.join(format!("{}.tar.gz", trace.trace_id));
    create_archive(bundle, &archive)?;
    let (bundle_sha256, total_bytes) = file_sha256(&archive)?;
    let part_count = total_bytes.div_ceil(PART_BYTES);
    let model = trace
        .inference_calls
        .values()
        .next()
        .map(|call| call.model.as_str())
        .or_else(|| {
            trace
                .threads
                .get(&trace.root_thread_id)
                .and_then(|thread| thread.default_model.as_deref())
        });
    let ingest_url = ingest_url();
    let response = client
        .post(format!("{ingest_url}/sessions"))
        .bearer_auth(access_token)
        .json(&CreateSessionRequest {
            trace_id: &trace.trace_id,
            rollout_id: &trace.rollout_id,
            root_thread_id: &trace.root_thread_id,
            schema_version: trace.schema_version,
            capture_policy_version: consent.policy_version,
            consent_accepted_at: &consent.accepted_at,
            client_version: env!("CARGO_PKG_VERSION"),
            started_at: millis_rfc3339(trace.started_at_unix_ms)?,
            ended_at: trace.ended_at_unix_ms.map(millis_rfc3339).transpose()?,
            bundle_sha256: &bundle_sha256,
            total_bytes,
            part_count,
            model,
            repository_path: std::env::current_dir()
                .ok()
                .map(|path| path.to_string_lossy().into_owned()),
            repository_remote: git_value("remote", "get-url", "origin"),
            repository_commit: git_value("rev-parse", "HEAD", ""),
            metadata: serde_json::json!({
                "status": trace.status,
                "turn_count": trace.codex_turns.len(),
                "tool_call_count": trace.tool_calls.len(),
                "inference_call_count": trace.inference_calls.len(),
            }),
        })
        .send()
        .await?
        .error_for_status()?;
    let session = response.json::<SessionResponse>().await?;
    if session.status != "complete" {
        upload_parts(client, access_token, &ingest_url, &trace.trace_id, &archive).await?;
        client
            .post(format!("{ingest_url}/sessions/{}/complete", trace.trace_id))
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?;
    }
    std::fs::write(
        bundle.join(UPLOADED_MARKER),
        serde_json::to_vec_pretty(&UploadedMarker {
            trace_id: &trace.trace_id,
            bundle_sha256: &bundle_sha256,
            uploaded_at: now_rfc3339(),
            backend: "supabase:qudfqzabhkrhbeuvvqmt",
        })?,
    )?;
    std::fs::remove_file(archive)?;
    Ok(())
}

async fn upload_parts(
    client: &Client,
    access_token: &str,
    ingest_url: &str,
    trace_id: &str,
    archive: &Path,
) -> Result<()> {
    let mut file = File::open(archive)?;
    let mut part_number = 0_u64;
    loop {
        let mut bytes = vec![0_u8; PART_BYTES as usize];
        let size = file.read(&mut bytes)?;
        if size == 0 {
            break;
        }
        bytes.truncate(size);
        let part_sha256 = hex_digest(&bytes);
        client
            .put(format!(
                "{ingest_url}/sessions/{trace_id}/parts/{part_number}"
            ))
            .bearer_auth(access_token)
            .header("x-zipcode-part-sha256", part_sha256)
            .header("content-type", "application/octet-stream")
            .body(bytes)
            .send()
            .await?
            .error_for_status()?;
        part_number += 1;
    }
    Ok(())
}

fn create_archive(bundle: &Path, destination: &Path) -> Result<()> {
    let file = File::create(destination)?;
    let gzip = GzEncoder::new(file, Compression::default());
    let mut archive = tar::Builder::new(gzip);
    archive.follow_symlinks(false);
    archive.append_dir_all("trace", bundle)?;
    let gzip = archive.into_inner()?;
    gzip.finish()?;
    Ok(())
}

fn file_sha256(path: &Path) -> Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        hasher.update(&buffer[..size]);
        total += size as u64;
    }
    Ok((format!("{:x}", hasher.finalize()), total))
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn millis_rfc3339(millis: i64) -> Result<String> {
    let timestamp = DateTime::<Utc>::from_timestamp_millis(millis)
        .context("trace timestamp is outside the supported range")?;
    Ok(timestamp.to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn ingest_url() -> String {
    std::env::var("ZIPCODE_TRACE_INGEST_URL")
        .unwrap_or_else(|_| DEFAULT_INGEST_URL.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn git_value(command: &str, argument: &str, trailing: &str) -> Option<String> {
    let mut process = std::process::Command::new("git");
    process.arg(command).arg(argument);
    if !trailing.is_empty() {
        process.arg(trailing);
    }
    let output = process.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
#[path = "trace_upload_tests.rs"]
mod tests;
