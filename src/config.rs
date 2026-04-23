use std::path::PathBuf;

use anyhow::{Context, Result, bail};

#[derive(Clone, Debug)]
pub struct Config {
    pub data_dir: PathBuf,
    pub github_client_id: String,
    pub github_client_secret: String,
    pub session_secret: Vec<u8>,
    pub base_url: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let data_dir = PathBuf::from(
            std::env::var("DATA_DIR").context("DATA_DIR env var missing")?,
        );
        let github_client_id =
            std::env::var("GITHUB_CLIENT_ID").context("GITHUB_CLIENT_ID env var missing")?;
        let github_client_secret = std::env::var("GITHUB_CLIENT_SECRET")
            .context("GITHUB_CLIENT_SECRET env var missing")?;
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

        Ok(Self {
            data_dir,
            github_client_id,
            github_client_secret,
            session_secret,
            base_url,
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
        }
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
