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

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let jwt_secret = std::env::var("JWT_SECRET")
            .ok()
            .filter(|v| v.len() >= 32)
            .unwrap_or_else(|| {
                // Deterministic dev fallback keeps local dev easy, but is
                // refused when RTERM_ENV=production.
                "dev-only-secret-change-me-0123456789abcdef".to_string()
            });

        let dev_auth = env_or("AUTH_DEV_MODE", "false") == "true";
        let production = env_or("RTERM_ENV", "development") == "production";
        if production && jwt_secret.starts_with("dev-only") {
            anyhow::bail!("JWT_SECRET must be set (>= 32 chars) in production");
        }
        if production && dev_auth {
            anyhow::bail!("AUTH_DEV_MODE must be off in production");
        }

        Ok(Self {
            bind: env_or("BIND_ADDR", "0.0.0.0:8080"),
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
