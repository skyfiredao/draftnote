use crate::secret;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_SYNC_INTERVAL_SECONDS: i64 = 10;

fn is_empty_string(s: &String) -> bool {
    s.is_empty()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub repo_url: String,
    #[serde(default, skip_serializing_if = "is_empty_string")]
    pub server_url: String,
    #[serde(default)]
    pub username: String,
    #[serde(skip)]
    pub token: String,
    #[serde(default, skip_serializing_if = "is_empty_string")]
    pub token_secret: String,
    #[serde(default)]
    pub use_api: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub sync_interval_seconds: i64,
    #[serde(default, skip_serializing_if = "is_empty_string")]
    pub mismatch_ack_server_url: String,
}

impl Config {
    pub fn interval_seconds(&self) -> i64 {
        if self.sync_interval_seconds <= 0 {
            DEFAULT_SYNC_INTERVAL_SECONDS
        } else {
            self.sync_interval_seconds
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicConfig {
    pub repo_url: String,
    pub server_url: String,
    pub username: String,
    pub use_api: bool,
    pub has_token: bool,
    pub sync_interval_seconds: i64,
}

impl Config {
    pub fn public(&self) -> PublicConfig {
        PublicConfig {
            repo_url: self.repo_url.clone(),
            server_url: self.server_url.clone(),
            username: self.username.clone(),
            use_api: self.use_api,
            has_token: !self.token.is_empty(),
            sync_interval_seconds: self.sync_interval_seconds,
        }
    }
}

pub fn config_path(root: &Path) -> PathBuf {
    root.join("config.json")
}

pub fn load_config(root: &Path) -> Config {
    let b = match fs::read(config_path(root)) {
        Ok(b) => b,
        Err(_) => {
            return Config {
                sync_interval_seconds: DEFAULT_SYNC_INTERVAL_SECONDS,
                use_api: true,
                ..Config::default()
            }
        }
    };
    let mut c: Config = serde_json::from_slice(&b).unwrap_or_default();
    if c.sync_interval_seconds <= 0 {
        c.sync_interval_seconds = DEFAULT_SYNC_INTERVAL_SECONDS;
    }
    if !c.token_secret.is_empty() {
        if let Ok(tok) = secret::open_with(&key_from_username(&c.username), &c.token_secret) {
            c.token = tok;
        }
    }
    c
}

fn key_from_username(username: &str) -> [u8; 32] {
    Sha256::digest(username.as_bytes()).into()
}

pub fn save_config(root: &Path, cfg: &Config) -> std::io::Result<()> {
    fs::create_dir_all(root)?;
    let mut c = cfg.clone();
    c.token_secret = secret::seal_with(&key_from_username(&cfg.username), &cfg.token)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let b = serde_json::to_vec_pretty(&c)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    write_private(&config_path(root), &b)
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    fs::write(path, bytes)
}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidRepoUrl;

impl std::fmt::Display for InvalidRepoUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid repo URL")
    }
}

pub fn parse_repo_url(
    repo_url: &str,
    use_api: bool,
) -> Result<(String, String, String), InvalidRepoUrl> {
    let s = repo_url.trim();
    let s = s.strip_suffix(".git").unwrap_or(s);
    if s.is_empty() {
        return Err(InvalidRepoUrl);
    }
    let u = url::Url::parse(s).map_err(|_| InvalidRepoUrl)?;
    if u.scheme() != "http" && u.scheme() != "https" {
        return Err(InvalidRepoUrl);
    }
    let host = match u.host_str() {
        Some(h) if !h.is_empty() => h.to_string(),
        _ => return Err(InvalidRepoUrl),
    };
    let path = u.path().trim_matches('/');
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(InvalidRepoUrl);
    }
    let owner = parts[0].to_string();
    let repo = parts[1].to_string();
    let host_l = host.to_lowercase();
    let base = if use_api && (host_l == "github.com" || host_l == "www.github.com") {
        "https://api.github.com".to_string()
    } else {
        let port = u.port().map(|p| format!(":{p}")).unwrap_or_default();
        format!("{}://{}{}", u.scheme(), host, port)
    };
    Ok((base, owner, repo))
}

pub fn normalize_repo_url(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_suffix(".git").unwrap_or(s);
    s.trim_end_matches('/').to_lowercase()
}

pub fn preflight(cfg: &Config) -> Result<(), String> {
    if cfg.repo_url.is_empty() || cfg.base_url.is_empty() || cfg.owner.is_empty() || cfg.repo.is_empty()
    {
        return Err("Missing repo URL".to_string());
    }
    if cfg.token.is_empty() {
        return Err("Missing token".to_string());
    }
    if cfg.username.is_empty() {
        return Err("Missing username".to_string());
    }
    Ok(())
}

pub fn extract_status(msg: &str) -> String {
    let i = match msg.find("status ") {
        Some(i) => i,
        None => return msg.to_string(),
    };
    let rest = &msg[i + "status ".len()..];
    match rest.find([' ', ':']) {
        Some(end) => rest[..end].to_string(),
        None => rest.to_string(),
    }
}

pub fn map_validate_error(base_url: &str, msg: &str) -> String {
    let lower = msg.to_lowercase();
    if lower.contains("status 401") {
        return "Auth rejected: check username/token".to_string();
    }
    if lower.contains("status 403") {
        return "Forbidden: token lacks repo access".to_string();
    }
    if (lower.contains("create tree:") || lower.contains("create commit:"))
        && lower.contains("status 404")
    {
        return "Repo not found: check owner/repo in URL".to_string();
    }
    if lower.contains("status 5") {
        return format!("Server error: {}", extract_status(msg));
    }
    if lower.contains("create ref")
        || lower.contains("create tree")
        || lower.contains("create commit")
    {
        return format!("Cannot create draftnote branch: {msg}");
    }
    if lower.contains("no such host")
        || lower.contains("dial tcp")
        || lower.contains("connection refused")
        || lower.contains("timeout")
        || lower.contains("dns error")
        || lower.contains("error trying to connect")
    {
        let host = url::Url::parse(base_url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()))
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| base_url.to_string());
        return format!("Cannot reach server: {host}");
    }
    msg.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_github_api_rewrite() {
        let (base, owner, repo) =
            parse_repo_url("https://github.com/alice/notes", true).unwrap();
        assert_eq!(base, "https://api.github.com");
        assert_eq!(owner, "alice");
        assert_eq!(repo, "notes");
    }

    #[test]
    fn parse_github_no_rewrite_when_api_off() {
        let (base, _, _) = parse_repo_url("https://github.com/alice/notes", false).unwrap();
        assert_eq!(base, "https://github.com");
    }

    #[test]
    fn parse_self_hosted_keeps_host_and_port() {
        let (base, owner, repo) =
            parse_repo_url("http://10.0.1.244:8015/bob/repo", true).unwrap();
        assert_eq!(base, "http://10.0.1.244:8015");
        assert_eq!(owner, "bob");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn parse_strips_dot_git() {
        let (_, owner, repo) =
            parse_repo_url("https://github.com/alice/notes.git", true).unwrap();
        assert_eq!(owner, "alice");
        assert_eq!(repo, "notes");
    }

    #[test]
    fn parse_rejects_bad() {
        assert!(parse_repo_url("", true).is_err());
        assert!(parse_repo_url("ftp://x/y/z", true).is_err());
        assert!(parse_repo_url("https://github.com/onlyowner", true).is_err());
        assert!(parse_repo_url("not a url", true).is_err());
    }

    #[test]
    fn normalize_matches_variants() {
        assert_eq!(
            normalize_repo_url("https://github.com/A/B/"),
            "https://github.com/a/b"
        );
        assert_eq!(
            normalize_repo_url("https://github.com/A/B.git"),
            "https://github.com/a/b"
        );
    }

    #[test]
    fn extract_status_reads_code() {
        assert_eq!(extract_status("put x: status 503: boom"), "503");
        assert_eq!(extract_status("plain message"), "plain message");
    }

    #[test]
    fn map_validate_error_classifies() {
        assert_eq!(
            map_validate_error("http://h", "get: status 401: nope"),
            "Auth rejected: check username/token"
        );
        assert_eq!(
            map_validate_error("http://h", "status 403 forbidden"),
            "Forbidden: token lacks repo access"
        );
        assert_eq!(
            map_validate_error("http://h", "create tree: status 404"),
            "Repo not found: check owner/repo in URL"
        );
        assert!(map_validate_error("http://h", "x status 500 y").starts_with("Server error"));
        assert_eq!(
            map_validate_error("http://10.0.1.244:8015", "dns error: no such host"),
            "Cannot reach server: 10.0.1.244"
        );
    }

    #[test]
    fn config_save_load_round_trips_token() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config {
            repo_url: "https://github.com/a/b".to_string(),
            username: "alice".to_string(),
            token: "ghp_secret".to_string(),
            use_api: true,
            base_url: "https://api.github.com".to_string(),
            owner: "a".to_string(),
            repo: "b".to_string(),
            sync_interval_seconds: 10,
            ..Config::default()
        };
        save_config(dir.path(), &cfg).unwrap();
        let raw = fs::read_to_string(config_path(dir.path())).unwrap();
        assert!(!raw.contains("ghp_secret"), "token must not be stored in plaintext");
        let loaded = load_config(dir.path());
        assert_eq!(loaded.token, "ghp_secret");
        assert!(loaded.public().has_token);
    }

    #[test]
    fn load_missing_config_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let c = load_config(dir.path());
        assert_eq!(c.sync_interval_seconds, DEFAULT_SYNC_INTERVAL_SECONDS);
        assert!(c.use_api);
        assert!(!c.public().has_token);
    }

    #[test]
    fn preflight_requires_username_when_token_present() {
        let full = Config {
            repo_url: "https://github.com/a/b".to_string(),
            base_url: "https://api.github.com".to_string(),
            owner: "a".to_string(),
            repo: "b".to_string(),
            token: "tok".to_string(),
            username: "alice".to_string(),
            use_api: true,
            ..Config::default()
        };
        assert!(preflight(&full).is_ok());
        assert_eq!(
            preflight(&Config { username: String::new(), ..full.clone() }),
            Err("Missing username".to_string())
        );
        assert_eq!(
            preflight(&Config { token: String::new(), ..full.clone() }),
            Err("Missing token".to_string())
        );
        assert_eq!(
            preflight(&Config { repo_url: String::new(), ..full }),
            Err("Missing repo URL".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn config_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        save_config(dir.path(), &Config::default()).unwrap();
        let mode = fs::metadata(config_path(dir.path()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "config.json must be owner-only");
    }
}
