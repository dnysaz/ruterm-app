//! Google Sign-In verification.
//!
//! Two flows are supported:
//!  * mobile / native SDK: the app obtains an `id_token` and posts it here;
//!  * desktop: authorization-code flow with a loopback redirect, the app posts
//!    the `code` here and we exchange it with the client secret (which never
//!    ships inside the app).

use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::error::ApiError;

const CERTS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";

static JWKS_CACHE: Mutex<Option<(Instant, Arc<JwkSet>)>> = Mutex::const_new(None);

#[derive(Debug, Clone, Deserialize)]
pub struct GoogleClaims {
    pub sub: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub email_verified: Option<bool>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub picture: Option<String>,
}

async fn jwks(http: &reqwest::Client) -> Result<Arc<JwkSet>, ApiError> {
    let mut cache = JWKS_CACHE.lock().await;
    if let Some((fetched_at, set)) = cache.as_ref() {
        if fetched_at.elapsed() < Duration::from_secs(1800) {
            return Ok(set.clone());
        }
    }
    let set: JwkSet = http
        .get(CERTS_URL)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(e.to_string()))?
        .json()
        .await
        .map_err(|e| ApiError::Upstream(e.to_string()))?;
    let set = Arc::new(set);
    *cache = Some((Instant::now(), set.clone()));
    Ok(set)
}

pub async fn verify_id_token(
    http: &reqwest::Client,
    client_id: &str,
    id_token: &str,
    allowed_domains: &[String],
) -> Result<GoogleClaims, ApiError> {
    let header = decode_header(id_token)?;
    let set = jwks(http).await?;
    let jwk = set
        .keys
        .iter()
        .find(|k| header.kid.is_some() && k.common.key_id.as_deref() == header.kid.as_deref())
        .or_else(|| set.keys.first())
        .ok_or_else(|| ApiError::Upstream("google jwks is empty".into()))?;

    let mut validation = Validation::new(header.alg);
    validation.set_audience(&[client_id]);
    validation.set_issuer(&["accounts.google.com", "https://accounts.google.com"]);

    let key = DecodingKey::from_jwk(jwk).map_err(|e| ApiError::Upstream(e.to_string()))?;
    let data = decode::<GoogleClaims>(id_token, &key, &validation)
        .map_err(|e| ApiError::Unauthorized(format!("invalid google token: {e}")))?;

    let claims = data.claims;
    if claims.email_verified == Some(false) {
        return Err(ApiError::Unauthorized("google email is not verified".into()));
    }
    if claims.email.is_empty() {
        return Err(ApiError::Unauthorized("google token has no email".into()));
    }
    if !allowed_domains.is_empty() {
        let domain = claims
            .email
            .rsplit_once('@')
            .map(|(_, d)| d.to_lowercase())
            .unwrap_or_default();
        if !allowed_domains.contains(&domain) {
            return Err(ApiError::Unauthorized(format!(
                "domain {domain} is not allowed"
            )));
        }
    }
    Ok(claims)
}

fn decode_header(token: &str) -> Result<jsonwebtoken::Header, ApiError> {
    let header = jsonwebtoken::decode_header(token)
        .map_err(|e| ApiError::Unauthorized(format!("invalid google token: {e}")))?;
    if header.alg != Algorithm::RS256 {
        return Err(ApiError::Unauthorized("unexpected google token algorithm".into()));
    }
    Ok(header)
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
}

#[derive(Deserialize)]
struct UserInfo {
    sub: String,
    email: String,
    #[serde(default)]
    email_verified: Option<bool>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    picture: Option<String>,
}

/// Exchanges an OAuth authorization code (desktop/loopback and web clients).
pub async fn exchange_code(
    http: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
) -> Result<GoogleClaims, ApiError> {
    let res = http
        .post(TOKEN_URL)
        .form(&[
            ("code", code),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|e| ApiError::Upstream(e.to_string()))?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(ApiError::Unauthorized(format!(
            "google code exchange failed ({status}): {body}"
        )));
    }

    let token: TokenResponse = res
        .json()
        .await
        .map_err(|e| ApiError::Upstream(e.to_string()))?;

    if let Some(id_token) = token.id_token {
        // Re-verify rather than trusting the transport alone.
        return verify_id_token(http, client_id, &id_token, &[]).await;
    }

    let access = token
        .access_token
        .ok_or_else(|| ApiError::Upstream("google returned no tokens".into()))?;
    let info: UserInfo = http
        .get(USERINFO_URL)
        .bearer_auth(access)
        .send()
        .await
        .map_err(|e| ApiError::Upstream(e.to_string()))?
        .json()
        .await
        .map_err(|e| ApiError::Upstream(e.to_string()))?;

    Ok(GoogleClaims {
        sub: info.sub,
        email: info.email,
        email_verified: info.email_verified,
        name: info.name,
        picture: info.picture,
    })
}

/// URL the desktop app should open in the system browser.
pub fn authorization_url(client_id: &str, redirect_uri: &str, state: &str) -> String {
    format!(
        "https://accounts.google.com/o/oauth2/v2/auth?\
         client_id={client_id}&\
         redirect_uri={redirect}&\
         response_type=code&\
         scope=openid%20email%20profile&\
         access_type=offline&\
         prompt=select_account&\
         state={state}",
        redirect = urlencoding(redirect_uri)
    )
}

fn urlencoding(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
