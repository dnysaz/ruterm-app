//! Google sign-in + Drive access, without any backend of our own.
//!
//! * Desktop: OAuth 2.0 authorization code with PKCE and a loopback redirect
//!   (RFC 8252). Works with a "Desktop app" OAuth client, so there is **no
//!   client secret** to protect and no server to run.
//! * Mobile: the native Google Sign-In SDK obtains the access token; the app
//!   passes it to [`crate::drive::DriveClient`].
//!
//! Vault secrets are encrypted client-side before upload to the hidden
//! `appDataFolder` of the user's own Drive; host metadata in the document is
//! plain JSON and readable by anyone with access to that Drive file.

use std::time::Duration;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64URL, Engine};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub const DRIVE_APPDATA_SCOPE: &str = "https://www.googleapis.com/auth/drive.appdata";
pub const DEFAULT_SCOPES: &str =
    "openid email profile https://www.googleapis.com/auth/drive.appdata";
/// Identity-only scopes: enough for the bridge bridge login (email + profile).
pub const IDENTITY_SCOPES: &str = "openid email profile";

#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("google returned an error: {0}")]
    Google(String),
    #[error("oauth error: {0}")]
    Protocol(String),
    #[error("login timed out")]
    Timeout,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct GoogleAccount {
    pub sub: String,
    pub email: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub picture: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct GoogleTokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Seconds until the access token expires.
    #[serde(default)]
    pub expires_in: u64,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
}

// F-12: redact token material in Debug output.
impl std::fmt::Debug for GoogleTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleTokens")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "<redacted>"))
            .field("expires_in", &self.expires_in)
            .field("scope", &self.scope)
            .field("id_token", &self.id_token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct GoogleSession {
    pub tokens: GoogleTokens,
    pub account: GoogleAccount,
}

impl std::fmt::Debug for GoogleSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleSession")
            .field("tokens", &self.tokens)
            .field("account", &self.account)
            .finish()
    }
}

#[derive(Deserialize)]
struct TokenEndpointResponse {
    access_token: String,
    #[serde(default)]
    expires_in: u64,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
}

#[derive(Deserialize)]
struct IdClaims {
    sub: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    picture: Option<String>,
}

#[derive(Deserialize)]
struct UserInfo {
    sub: String,
    email: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    picture: Option<String>,
}

fn verifier() -> String {
    let mut raw = [0u8; 48];
    rand::thread_rng().fill_bytes(&mut raw);
    B64URL.encode(raw)
}

fn challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    B64URL.encode(digest)
}

/// The artifacts of a completed loopback authorization-code flow.
pub struct LoopbackFlow {
    pub code: String,
    pub code_verifier: String,
    pub redirect_uri: String,
}

/// Runs the RFC 8252 loopback flow (browser + PKCE) and returns the raw
/// authorization code instead of exchanging it. Callers that talk to the
/// RuTerm bridge forward the code to it; it verifies identity server-side.
pub async fn authorize_loopback(
    client_id: &str,
    scopes: &str,
    timeout: Duration,
) -> Result<LoopbackFlow, OAuthError> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    let code_verifier = verifier();
    let state = uuid::Uuid::new_v4().simple().to_string();
    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?\
         client_id={}&\
         redirect_uri={}&\
         response_type=code&\
         scope={}&\
         code_challenge={}&\
         code_challenge_method=S256&\
         access_type=offline&\
         prompt=consent&\
         include_granted_scopes=true&\
         state={}",
        urlencode(client_id),
        urlencode(&redirect_uri),
        urlencode(scopes),
        challenge(&code_verifier),
        state
    );

    tokio::task::spawn_blocking(move || open::that(auth_url))
        .await
        .map_err(|e| OAuthError::Protocol(e.to_string()))?
        .map_err(|e| OAuthError::Protocol(format!("cannot open browser: {e}")))?;

    let (code, returned_state) = tokio::time::timeout(timeout, wait_for_code(listener))
        .await
        .map_err(|_| OAuthError::Timeout)??;

    // State is mandatory: an empty or missing value must never bypass CSRF
    // validation (F-15).
    if returned_state != state {
        return Err(OAuthError::Protocol("oauth state mismatch".into()));
    }

    Ok(LoopbackFlow {
        code,
        code_verifier,
        redirect_uri,
    })
}

/// Full desktop flow: open browser, wait for loopback redirect, exchange code.
pub async fn login_desktop(
    client_id: &str,
    timeout: Duration,
) -> Result<GoogleSession, OAuthError> {
    let flow = authorize_loopback(client_id, DEFAULT_SCOPES, timeout).await?;
    let http = reqwest::Client::new();
    let res = http
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", client_id),
            ("code", flow.code.as_str()),
            ("code_verifier", flow.code_verifier.as_str()),
            ("redirect_uri", flow.redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await?;

    finish_token_response(&http, res).await
}

/// Refreshes an access token. `refresh_token` comes from the native SDK or
/// from [`login_desktop`].
pub async fn refresh_access_token(
    client_id: &str,
    refresh_token: &str,
) -> Result<GoogleTokens, OAuthError> {
    let http = reqwest::Client::new();
    let res = http
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", client_id),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await?;

    let status = res.status();
    let text = res.text().await?;
    if !status.is_success() {
        return Err(OAuthError::Google(format!("{status}: {text}")));
    }
    let token: TokenEndpointResponse = serde_json::from_str(&text)?;
    Ok(GoogleTokens {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_in: token.expires_in,
        scope: token.scope,
        id_token: token.id_token,
    })
}

async fn finish_token_response(
    http: &reqwest::Client,
    res: reqwest::Response,
) -> Result<GoogleSession, OAuthError> {
    let status = res.status();
    let text = res.text().await?;
    if !status.is_success() {
        return Err(OAuthError::Google(format!("{status}: {text}")));
    }
    let token: TokenEndpointResponse =
        serde_json::from_str(&text).map_err(|e| OAuthError::Protocol(e.to_string()))?;

    let account = match token.id_token.as_deref() {
        Some(id_token) => account_from_id_token(id_token)?,
        None => {
            let info: UserInfo = http
                .get("https://openidconnect.googleapis.com/v1/userinfo")
                .bearer_auth(&token.access_token)
                .send()
                .await?
                .json()
                .await?;
            GoogleAccount {
                sub: info.sub,
                email: info.email,
                name: info.name,
                picture: info.picture,
            }
        }
    };

    Ok(GoogleSession {
        tokens: GoogleTokens {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_in: token.expires_in,
            scope: token.scope,
            id_token: token.id_token,
        },
        account,
    })
}

/// Decodes the (already trusted, received over TLS straight from Google)
/// id_token payload. Signature checking is the server-side job in flows that
/// involve a backend; here we never trust it for authorisation.
pub fn account_from_id_token(id_token: &str) -> Result<GoogleAccount, OAuthError> {
    let payload = id_token
        .split('.')
        .nth(1)
        .ok_or_else(|| OAuthError::Protocol("malformed id_token".into()))?;
    let bytes = B64URL
        .decode(payload)
        .map_err(|e| OAuthError::Protocol(format!("malformed id_token: {e}")))?;
    let claims: IdClaims = serde_json::from_slice(&bytes)?;
    Ok(GoogleAccount {
        sub: claims.sub,
        email: claims.email,
        name: claims.name,
        picture: claims.picture,
    })
}

async fn wait_for_code(listener: TcpListener) -> Result<(String, String), OAuthError> {
    loop {
        let (mut socket, _) = listener.accept().await?;
        let mut buf = vec![0u8; 8192];
        let n = socket.read(&mut buf).await?;
        let request = String::from_utf8_lossy(&buf[..n]);
        let path = request
            .lines()
            .next()
            .unwrap_or_default()
            .split_whitespace()
            .nth(1)
            .unwrap_or("/")
            .to_string();

        if !path.starts_with("/callback") {
            let _ = socket
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                .await;
            let _ = socket.shutdown().await;
            continue;
        }

        let query = path.split_once('?').map(|(_, q)| q).unwrap_or_default();
        let mut code = String::new();
        let mut state = String::new();
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            match k {
                "code" => code = percent_decode(v),
                "state" => state = percent_decode(v),
                "error" => {
                    // Never reflect the attacker-controlled `error` value into
                    // the loopback HTML (F-16). Show a static message only.
                    respond(&mut socket, 400, "<h1>Login failed</h1><p>See the app for details.</p>").await;
                    return Err(OAuthError::Protocol("google authorization was denied".into()));
                }
                _ => {}
            }
        }

        if code.is_empty() {
            respond(&mut socket, 400, "<h1>Missing authorization code</h1>").await;
            continue;
        }

        let body = "<html><body style=\"font-family:system-ui;background:#0b0e14;color:#e6e6e6;\
                    display:flex;align-items:center;justify-content:center;height:100vh\">\
                    <div style=\"text-align:center\"><h2>Signed in to rterm</h2>\
                    <p>You can close this window and return to the app.</p></div></body></html>";
        respond(&mut socket, 200, body).await;
        return Ok((code, state));
    }
}

async fn respond(socket: &mut tokio::net::TcpStream, status: u16, body: &str) {
    let reason = if status == 200 { "OK" } else { "Bad Request" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                // Decode from bytes: slicing the &str at i+1..i+3 can land in
                // the middle of a multi-byte UTF-8 sequence and panic (F-47).
                let hi = hex_nibble(bytes[i + 1]);
                let lo = hex_nibble(bytes[i + 2]);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push((hi << 4) | lo);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn urlencode(value: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_handles_multibyte_without_panic() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("caf%C3%A9"), "café");
        // A multi-byte char immediately after a stray '%' used to panic.
        assert_eq!(percent_decode("%é"), "%é");
        assert_eq!(percent_decode("%ZZ"), "%ZZ");
    }
}
