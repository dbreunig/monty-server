use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::sandbox::SandboxConfig;

#[derive(Clone, Debug)]
pub struct Config {
    pub data_dir: PathBuf,
    pub github_client_id: String,
    pub github_client_secret: String,
    pub session_secret: Vec<u8>,
    pub base_url: String,
    /// OAuth2 authorize endpoint. Defaults to GitHub's; overridden in tests.
    pub github_auth_url: String,
    /// OAuth2 token endpoint. Defaults to GitHub's; overridden in tests.
    pub github_token_url: String,
    /// Base URL for the GitHub REST API (`/user`, `/user/emails`). Defaults to api.github.com.
    pub github_api_base: String,
    /// Optional admin email — if set with `admin_password`, a local admin user
    /// is seeded on startup. That user is auto-approved and can approve others.
    pub admin_email: Option<String>,
    pub admin_password: Option<String>,
    /// Monty worker pool settings.
    pub sandbox: SandboxConfig,
}

const DEFAULT_GITHUB_AUTH_URL: &str = "https://github.com/login/oauth/authorize";
const DEFAULT_GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const DEFAULT_GITHUB_API_BASE: &str = "https://api.github.com";

impl Config {
    pub fn from_env() -> Result<Self> {
        let data_dir =
            PathBuf::from(std::env::var("DATA_DIR").context("DATA_DIR env var missing")?);
        // GitHub OAuth is optional: empty strings (or unset) disable the
        // "Sign in with GitHub" button. Email/password auth still works.
        let github_client_id = std::env::var("GITHUB_CLIENT_ID").unwrap_or_default();
        let github_client_secret = std::env::var("GITHUB_CLIENT_SECRET").unwrap_or_default();
        let session_secret_str =
            std::env::var("SESSION_SECRET").context("SESSION_SECRET env var missing")?;
        let session_secret = session_secret_str.into_bytes();
        if session_secret.len() < 64 {
            bail!(
                "SESSION_SECRET must be at least 64 bytes, got {}",
                session_secret.len()
            );
        }
        let base_url = std::env::var("BASE_URL").context("BASE_URL env var missing")?;
        let admin_email = std::env::var("ADMIN_EMAIL").ok().filter(|s| !s.is_empty());
        let admin_password = std::env::var("ADMIN_PASSWORD").ok().filter(|s| !s.is_empty());
        if admin_email.is_some() != admin_password.is_some() {
            bail!("ADMIN_EMAIL and ADMIN_PASSWORD must be set together, or neither");
        }

        let mut sandbox = SandboxConfig {
            worker_bin: monty_worker_bin(),
            ..SandboxConfig::default()
        };
        if let Some(n) = env_usize("MONTY_MIN_WORKERS")? {
            sandbox.min_workers = n;
        }
        if let Some(n) = env_usize("MONTY_MAX_WORKERS")? {
            sandbox.max_workers = n;
        }

        Ok(Self {
            data_dir,
            github_client_id,
            github_client_secret,
            session_secret,
            base_url,
            github_auth_url: DEFAULT_GITHUB_AUTH_URL.to_string(),
            github_token_url: DEFAULT_GITHUB_TOKEN_URL.to_string(),
            github_api_base: DEFAULT_GITHUB_API_BASE.to_string(),
            admin_email,
            admin_password,
            sandbox,
        })
    }

    #[doc(hidden)]
    pub fn for_test(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            github_client_id: "test_cid".to_string(),
            github_client_secret: "test_csec".to_string(),
            session_secret: vec![0u8; 64],
            base_url: "http://localhost:3000".to_string(),
            github_auth_url: DEFAULT_GITHUB_AUTH_URL.to_string(),
            github_token_url: DEFAULT_GITHUB_TOKEN_URL.to_string(),
            github_api_base: DEFAULT_GITHUB_API_BASE.to_string(),
            admin_email: None,
            admin_password: None,
            sandbox: SandboxConfig {
                worker_bin: monty_worker_bin(),
                min_workers: 0,
                ..SandboxConfig::default()
            },
        }
    }
}

/// Where to find the `monty` worker binary: `MONTY_WORKER_BIN` if set, else a
/// `monty` next to this executable, else `monty` on `PATH`.
///
/// The `PATH` lookup happens here, in the server's environment: monty-pool
/// spawns workers with a cleared environment, so a bare `monty` would never
/// be found there.
pub fn monty_worker_bin() -> PathBuf {
    if let Some(path) = std::env::var_os("MONTY_WORKER_BIN").filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    if let Some(sibling) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("monty")))
        .filter(|p| p.is_file())
    {
        return sibling;
    }
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("monty"))
                .find(|p| p.is_file())
        })
        // Not found: keep the bare name so the startup error says what's missing.
        .unwrap_or_else(|| PathBuf::from("monty"))
}

fn env_usize(name: &str) -> Result<Option<usize>> {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => v
            .parse()
            .map(Some)
            .with_context(|| format!("{name} must be a non-negative integer")),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    // Serialize tests that mutate process-wide env vars.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn loads_from_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            env::set_var("DATA_DIR", "/tmp/monty-test");
            env::set_var("GITHUB_CLIENT_ID", "cid");
            env::set_var("GITHUB_CLIENT_SECRET", "csec");
            env::set_var("SESSION_SECRET", "a".repeat(64));
            env::set_var("BASE_URL", "http://localhost:3000");
        }
        let c = Config::from_env().unwrap();
        assert_eq!(c.data_dir.to_str().unwrap(), "/tmp/monty-test");
        assert_eq!(c.github_client_id, "cid");
        assert_eq!(c.github_client_secret, "csec");
        assert_eq!(c.session_secret.len(), 64);
        assert_eq!(c.base_url, "http://localhost:3000");
    }

    #[test]
    fn rejects_short_session_secret() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            env::set_var("DATA_DIR", "/tmp/monty-test");
            env::set_var("GITHUB_CLIENT_ID", "cid");
            env::set_var("GITHUB_CLIENT_SECRET", "csec");
            env::set_var("SESSION_SECRET", "tooshort");
            env::set_var("BASE_URL", "http://localhost:3000");
        }
        assert!(Config::from_env().is_err());
    }
}
