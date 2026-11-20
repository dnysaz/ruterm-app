use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Config {
    pub bind: String,
    pub database_url: String,
    pub jwt_secret: String,
    pub access_token_ttl: Duration,
    pub refresh_token_ttl_days: i64,
    pub google_client_id: Option<String>,
    pub google_client_secret: Option<String>,
    pub google_redirect_uri: Option<String>,
    pub google_allowed_domains: Vec<String>,
    /// Browser origins allowed to call the API. Tauri clients send no Origin
    /// header, so they are unaffected. Empty means "same-origin only".
    pub allowed_origins: Vec<String>,
    /// Allows `/v1/auth/dev` (no Google) - never enable in production.
    pub dev_auth: bool,
    pub max_vault_bytes: usize,
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Fails closed for missing, short, or obviously placeholder JWT secrets.
///
/// Security checks must not depend on `RTERM_ENV`: a forgotten env var must
/// never turn a public deployment into one signed with a published key.
fn validate_jwt_secret(secret: &str) -> anyhow::Result<()> {
    let trimmed = secret.trim();
    if trimmed.len() < 32 {
        anyhow::bail!("JWT_SECRET must be at least 32 bytes");
    }
    let lowered = trimmed.to_ascii_lowercase();
    const PLACEHOLDER_MARKERS: [&str; 8] = [
        "dev-only", "dev_only", "changeme", "change-me", "change_me",
        "example", "placeholder", "replace",
    ];
    if PLACEHOLDER_MARKERS.iter().any(|m| lowered.contains(m)) {
        anyhow::bail!("JWT_SECRET looks like a placeholder; generate a random value (e.g. openssl rand -hex 32)");
    }
    let distinct = trimmed.chars().collect::<std::collections::HashSet<_>>().len();
    if distinct < 8 {
        anyhow::bail!("JWT_SECRET has too little entropy");
    }
    Ok(())
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let jwt_secret = std::env::var("JWT_SECRET")
            .map_err(|_| anyhow::anyhow!("JWT_SECRET must be set (see README / docs)"))?;
        validate_jwt_secret(&jwt_secret)?;

        let dev_auth = env_or("AUTH_DEV_MODE", "false") == "true";
        let production = env_or("RTERM_ENV", "development") == "production";
        if production && dev_auth {
            anyhow::bail!("AUTH_DEV_MODE must be off in production");
        }

        Ok(Self {
            // Loopback by default; exposing the API is an explicit decision.
            bind: env_or("BIND_ADDR", "127.0.0.1:8080"),
            database_url: env_or("DATABASE_URL", "sqlite://data/rterm.db"),
            jwt_secret,
            access_token_ttl: Duration::from_secs(
                env_or("ACCESS_TOKEN_TTL_SECS", "3600").parse().unwrap_or(3600),
            ),
            refresh_token_ttl_days: env_or("REFRESH_TOKEN_TTL_DAYS", "60")
                .parse()
                .unwrap_or(60),
            google_client_id: std::env::var("GOOGLE_CLIENT_ID").ok().filter(|s| !s.is_empty()),
            google_client_secret: std::env::var("GOOGLE_CLIENT_SECRET")
                .ok()
                .filter(|s| !s.is_empty()),
            google_redirect_uri: std::env::var("GOOGLE_REDIRECT_URI").ok().filter(|s| !s.is_empty()),
            google_allowed_domains: env_or("GOOGLE_ALLOWED_DOMAINS", "")
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
            allowed_origins: env_or(
                "ALLOWED_ORIGINS",
                "tauri://localhost,http://tauri.localhost,http://127.0.0.1:1420",
            )
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
            dev_auth,
            max_vault_bytes: env_or("MAX_VAULT_BYTES", "8388608").parse().unwrap_or(8 * 1024 * 1024),
        })
    }

    pub fn google(&self) -> anyhow::Result<(&str, &str)> {
        match (&self.google_client_id, &self.google_client_secret) {
            (Some(id), Some(secret)) => Ok((id, secret)),
            _ => anyhow::bail!(
                "GOOGLE_CLIENT_ID / GOOGLE_CLIENT_SECRET are not configured; \
                 use AUTH_DEV_MODE=true for local development"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::validate_jwt_secret;

    #[test]
    fn rejects_missing_short_and_placeholder_secrets() {
        assert!(validate_jwt_secret("").is_err());
        assert!(validate_jwt_secret("too-short").is_err());
        assert!(validate_jwt_secret("dev-only-secret-change-me-0123456789abcdef").is_err());
        assert!(validate_jwt_secret("replace-with-random-hex-0123456789abcdef0123").is_err());
        assert!(validate_jwt_secret("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").is_err());
    }

    #[test]
    fn accepts_random_high_entropy_secret() {
        assert!(validate_jwt_secret("8f14e45fceea167a5a36dedd4bea2543c9b0d7a1e6f2c38495bd8a0f1c2e3d4f").is_ok());
    }
}
