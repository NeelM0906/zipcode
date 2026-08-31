use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use codex_keyring_store::DefaultKeyringStore;
use codex_keyring_store::KeyringStore;
use reqwest::StatusCode;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

const DEFAULT_API_URL: &str = "https://olympustest.ngrok.pro/v1";
const KEYRING_SERVICE: &str = "ZIPCODE";
const KEYRING_ACCOUNT: &str = "team-session";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Session {
    access_token: String,
    refresh_token: String,
    expires_at: u64,
    github_login: String,
}

#[derive(Debug, Deserialize)]
struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Debug, Deserialize)]
struct GithubToken {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Serialize)]
struct ExchangeRequest<'a> {
    github_token: &'a str,
}

#[derive(Debug, Serialize)]
struct RefreshRequest<'a> {
    refresh_token: &'a str,
}

#[derive(Debug, Deserialize)]
struct SessionResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
    github_login: String,
}

#[derive(Debug, Serialize)]
struct LogoutRequest<'a> {
    refresh_token: &'a str,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("ZIPCODE: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let mut args = std::env::args_os();
    let _program = args.next();
    let rest: Vec<OsString> = args.collect();

    match rest.first().and_then(|arg| arg.to_str()) {
        Some("--version" | "-V") if rest.len() == 1 => {
            println!("zip-code {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("login") if rest.get(1).and_then(|arg| arg.to_str()) == Some("status") => {
            if rest.len() == 2 {
                login_status()
            } else {
                bail!("usage: zip-code login status")
            }
        }
        Some("login") if help_requested(&rest[1..]) => {
            print_login_help();
            Ok(())
        }
        Some("login") if rest.len() == 1 => login().await,
        Some("login") => bail!("usage: zip-code login [status]"),
        Some("logout") if help_requested(&rest[1..]) => {
            println!("Remove the saved ZIPCODE session.\n\nUsage: zip-code logout");
            Ok(())
        }
        Some("logout") if rest.len() == 1 => logout().await,
        Some("logout") => bail!("usage: zip-code logout"),
        Some("update") if help_requested(&rest[1..]) => {
            print_update_help();
            Ok(())
        }
        Some("update") if rest.len() == 1 => {
            print_update_help();
            Ok(())
        }
        Some("update") => bail!("usage: zip-code update"),
        Some("help") if rest.get(1).and_then(|arg| arg.to_str()) == Some("login") => {
            print_login_help();
            Ok(())
        }
        Some("help") if rest.get(1).and_then(|arg| arg.to_str()) == Some("logout") => {
            println!("Remove the saved ZIPCODE session.\n\nUsage: zip-code logout");
            Ok(())
        }
        Some("help") if rest.get(1).and_then(|arg| arg.to_str()) == Some("update") => {
            print_update_help();
            Ok(())
        }
        Some("auth-token") if rest.len() == 1 => {
            println!("{}", access_token().await?);
            Ok(())
        }
        Some("auth-token") => bail!("usage: zip-code auth-token"),
        _ => launch_core(&rest),
    }
}

fn help_requested(args: &[OsString]) -> bool {
    args.len() == 1 && matches!(args[0].to_str(), Some("--help" | "-h"))
}

fn print_login_help() {
    println!(
        "Sign in to the private ZIPCODE service with GitHub.\n\n\
         Usage:\n  zip-code login\n  zip-code login status"
    );
}

fn print_update_help() {
    println!(
        "Install the latest published ZIPCODE release with:\n\n\
         macOS/Linux:\n  curl -fsSL https://raw.githubusercontent.com/NeelM0906/zipcode/main/install.sh | sh\n\n\
         Windows PowerShell:\n  irm https://raw.githubusercontent.com/NeelM0906/zipcode/main/install.ps1 | iex"
    );
}

async fn login() -> Result<()> {
    let client_id = github_client_id()?;
    let client = reqwest::Client::builder()
        .user_agent("zipcode-cli")
        .build()?;
    let response = client
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .form(&[("client_id", client_id.as_str())])
        .send()
        .await?
        .error_for_status()?
        .json::<DeviceCode>()
        .await?;

    println!(
        "Open {} and enter code {}",
        response.verification_uri, response.user_code
    );
    if webbrowser::open(&response.verification_uri).is_err() {
        println!("Your browser could not be opened automatically.");
    }

    let deadline = now_epoch()?.saturating_add(response.expires_in);
    let mut interval = response.interval.max(5);
    let github_token = loop {
        if now_epoch()? >= deadline {
            bail!("GitHub device code expired; run `zip-code login` again");
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
        let token = client
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .form(&[
                ("client_id", client_id.as_str()),
                ("device_code", response.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json::<GithubToken>()
            .await?;
        if let Some(access_token) = token.access_token {
            break access_token;
        }
        match token.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval = interval.saturating_add(5),
            Some(error) => bail!(
                "GitHub login failed: {}",
                token.error_description.as_deref().unwrap_or(error)
            ),
            None => bail!("GitHub returned an invalid device-login response"),
        }
    };

    let session = client
        .post(format!("{}/auth/exchange", api_url()))
        .json(&ExchangeRequest {
            github_token: &github_token,
        })
        .send()
        .await?;
    if session.status() == StatusCode::FORBIDDEN {
        bail!("your GitHub account does not have a ZIPCODE invitation");
    }
    let session = session
        .error_for_status()?
        .json::<SessionResponse>()
        .await?;
    save_model_catalog(&client, &session.access_token).await?;
    let login = session.github_login.clone();
    save_session(session)?;
    println!("Logged in to ZIPCODE as @{login}");
    Ok(())
}

async fn access_token() -> Result<String> {
    let session = load_session()?.context("not logged in; run `zip-code login`")?;
    if session.expires_at > now_epoch()?.saturating_add(60) {
        return Ok(session.access_token);
    }

    let client = reqwest::Client::builder()
        .user_agent("zipcode-cli")
        .build()?;
    let response = client
        .post(format!("{}/auth/refresh", api_url()))
        .json(&RefreshRequest {
            refresh_token: &session.refresh_token,
        })
        .send()
        .await?;
    if response.status() == StatusCode::UNAUTHORIZED {
        bail!("session expired or invitation revoked; run `zip-code login`");
    }
    let refreshed = response
        .error_for_status()?
        .json::<SessionResponse>()
        .await?;
    let access_token = refreshed.access_token.clone();
    save_session(refreshed)?;
    Ok(access_token)
}

async fn logout() -> Result<()> {
    if let Some(session) = load_session()? {
        let client = reqwest::Client::builder()
            .user_agent("zipcode-cli")
            .build()?;
        let _response = client
            .post(format!("{}/auth/logout", api_url()))
            .json(&LogoutRequest {
                refresh_token: &session.refresh_token,
            })
            .send()
            .await;
    }
    delete_session()?;
    println!("Logged out of ZIPCODE");
    Ok(())
}

fn login_status() -> Result<()> {
    match load_session()? {
        Some(session) => println!("Logged in to ZIPCODE as @{}", session.github_login),
        None => println!("Not logged in"),
    }
    Ok(())
}

fn launch_core(args: &[OsString]) -> Result<()> {
    let current = std::env::current_exe().context("resolve ZIPCODE executable")?;
    let core = sibling_core(&current);
    if !core.is_file() {
        bail!("missing runtime at {}; reinstall ZIPCODE", core.display());
    }
    let home = zipcode_home()?;
    std::fs::create_dir_all(&home)?;
    ensure_config(&home)?;
    let mut command = Command::new(core);
    command.args(args).env("CODEX_HOME", home);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        Err(error).context("launch ZIPCODE runtime")
    }
    #[cfg(not(unix))]
    {
        let status = command.status().context("launch ZIPCODE runtime")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

async fn save_model_catalog(client: &reqwest::Client, access_token: &str) -> Result<()> {
    let catalog = client
        .get(format!(
            "{}/models?client_version={}",
            api_url(),
            env!("CARGO_PKG_VERSION")
        ))
        .bearer_auth(access_token)
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    let path = zipcode_home()?.join("models.json");
    std::fs::write(path, serde_json::to_vec_pretty(&catalog)?)?;
    Ok(())
}

fn ensure_config(home: &Path) -> Result<()> {
    let config_path = home.join("config.toml");
    let executable = std::env::current_exe().context("resolve ZIPCODE executable")?;
    let command = toml_string(&executable.to_string_lossy());
    let catalog = toml_string(&home.join("models.json").to_string_lossy());
    if config_path.exists() {
        let existing = std::fs::read_to_string(&config_path)?;
        if let Some(migrated) = migrate_legacy_config(&existing, &command, &catalog) {
            let backup = home.join("config.toml.pre-v0.1");
            if !backup.exists() {
                std::fs::copy(&config_path, &backup)?;
                set_private_permissions(&backup)?;
            }
            std::fs::write(&config_path, migrated)?;
            set_private_permissions(&config_path)?;
        }
        return Ok(());
    }
    let api = toml_string(&api_url());
    let config = format!(
        r#"model = "Qwen/Qwen3.8-Flash-Next"
model_provider = "zipcode_team"
model_reasoning_effort = "xhigh"
model_reasoning_summary = "none"
approval_policy = "on-request"
sandbox_mode = "workspace-write"
model_catalog_json = "{catalog}"

[model_providers.zipcode_team]
name = "ZIPCODE Private Coding Cloud"
base_url = "{api}"
wire_api = "responses"
request_max_retries = 2
stream_max_retries = 2
stream_idle_timeout_ms = 1800000
auth = {{ command = "{command}", args = ["auth-token"], refresh_interval_ms = 600000 }}

[features]
apps = false
browser_use = false
computer_use = false
image_generation = false
multi_agent = false
plugins = false
"#
    );
    std::fs::write(&config_path, config)?;
    set_private_permissions(&config_path)?;
    Ok(())
}

fn migrate_legacy_config(existing: &str, command: &str, catalog: &str) -> Option<String> {
    let mut in_zipcode_provider = false;
    let mut replaced_legacy_auth = false;
    let mut lines = Vec::new();
    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_zipcode_provider = trimmed == "[model_providers.zipcode_team]";
        }
        if in_zipcode_provider && trimmed.starts_with("env_key =") {
            lines.push(format!(
                "auth = {{ command = \"{command}\", args = [\"auth-token\"], refresh_interval_ms = 600000 }}"
            ));
            replaced_legacy_auth = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if !replaced_legacy_auth {
        return None;
    }

    if !lines
        .iter()
        .any(|line| line.trim_start().starts_with("model_catalog_json ="))
    {
        let first_table = lines
            .iter()
            .position(|line| line.trim_start().starts_with('['))
            .unwrap_or(lines.len());
        lines.insert(first_table, format!("model_catalog_json = \"{catalog}\""));
    }
    Some(lines.join("\n") + "\n")
}

fn set_private_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn sibling_core(current: &Path) -> PathBuf {
    let mut core = current.with_file_name("zip-code-core");
    if let Some(extension) = current.extension() {
        core.set_extension(extension);
    }
    core
}

fn github_client_id() -> Result<String> {
    std::env::var("ZIPCODE_GITHUB_CLIENT_ID")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| option_env!("ZIPCODE_GITHUB_CLIENT_ID").map(str::to_string))
        .context("this build has no GitHub OAuth client ID")
}

fn api_url() -> String {
    std::env::var("ZIPCODE_API_URL")
        .unwrap_or_else(|_| DEFAULT_API_URL.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn zipcode_home() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("ZIPCODE_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .context("could not find your home directory")?;
    Ok(PathBuf::from(home).join(".zipcode"))
}

fn session_path() -> Result<PathBuf> {
    Ok(zipcode_home()?.join("auth.json"))
}

fn load_session() -> Result<Option<Session>> {
    let keyring = DefaultKeyringStore;
    if let Ok(Some(value)) = keyring.load(KEYRING_SERVICE, KEYRING_ACCOUNT)
        && let Ok(session) = serde_json::from_str(&value)
    {
        return Ok(Some(session));
    }
    let path = session_path()?;
    match std::fs::read_to_string(path) {
        Ok(value) => Ok(Some(serde_json::from_str(&value)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn save_session(response: SessionResponse) -> Result<()> {
    let session: Session = response.into();
    let serialized = serde_json::to_string(&session)?;
    let keyring = DefaultKeyringStore;
    if keyring
        .save(KEYRING_SERVICE, KEYRING_ACCOUNT, &serialized)
        .is_ok()
    {
        let path = session_path()?;
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        return Ok(());
    }

    let path = session_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serialized)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn delete_session() -> Result<()> {
    let keyring = DefaultKeyringStore;
    let _deleted = keyring.delete(KEYRING_SERVICE, KEYRING_ACCOUNT);
    let path = session_path()?;
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn now_epoch() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

impl From<SessionResponse> for Session {
    fn from(response: SessionResponse) -> Self {
        Self {
            access_token: response.access_token,
            refresh_token: response.refresh_token,
            expires_at: now_epoch()
                .unwrap_or_default()
                .saturating_add(response.expires_in),
            github_login: response.github_login,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::migrate_legacy_config;
    use super::sibling_core;
    use super::toml_string;
    use std::path::Path;

    #[test]
    fn core_is_next_to_launcher() {
        assert_eq!(
            sibling_core(Path::new("/opt/zipcode/zip-code")),
            Path::new("/opt/zipcode/zip-code-core")
        );
        #[cfg(windows)]
        assert_eq!(
            sibling_core(Path::new(r"C:\ZIPCODE\zip-code.exe")),
            Path::new(r"C:\ZIPCODE\zip-code-core.exe")
        );
    }

    #[test]
    fn escapes_toml_paths() {
        assert_eq!(toml_string(r#"C:\ZIP "CODE""#), r#"C:\\ZIP \"CODE\""#);
    }

    #[test]
    fn migrates_legacy_team_auth_without_losing_user_settings() {
        let legacy = r#"model = "Qwen/Qwen3.8-Flash-Next"

[model_providers.zipcode_team]
base_url = "https://example.test/v1"
env_key = "ZIPCODE_API_KEY"
wire_api = "responses"

[projects."/work"]
trust_level = "trusted"
"#;
        let migrated = migrate_legacy_config(legacy, "/opt/zip-code", "/data/models.json")
            .expect("legacy config should migrate");
        assert!(!migrated.contains("env_key"));
        assert!(migrated.contains("command = \"/opt/zip-code\""));
        assert!(migrated.contains("model_catalog_json = \"/data/models.json\""));
        assert!(migrated.contains("[projects.\"/work\"]\ntrust_level = \"trusted\""));
    }

    #[test]
    fn leaves_nonlegacy_config_unchanged() {
        let current = "[model_providers.zipcode_team]\nauth = { command = \"zip-code\" }\n";
        assert!(migrate_legacy_config(current, "zip-code", "models.json").is_none());
    }
}
