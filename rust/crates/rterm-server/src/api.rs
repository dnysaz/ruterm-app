use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};

use axum::body::Body;
use axum::extract::{ConnectInfo, FromRequestParts, Path, Query, State};
use axum::http::{header, request::Parts, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::db::{self, UserRow};
use crate::error::{ApiError, ApiResult};
use crate::google;
use crate::Config;

/// Best-effort in-process throttle for `/v1/auth/*`. Slows online guessing on
/// a self-hosted instance; not a substitute for an edge WAF (F-14).
#[derive(Clone, Default)]
pub struct AuthLimiter {
    hits: Arc<Mutex<HashMap<String, (usize, Instant)>>>,
}

impl AuthLimiter {
    fn allow(&self, key: &str) -> bool {
        let mut hits = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let entry = hits.entry(key.to_string()).or_insert((0, now));
        if now.duration_since(entry.1) >= StdDuration::from_secs(60) {
            *entry = (1, now);
            return true;
        }
        entry.0 += 1;
        entry.0 <= 10
    }
}

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub db: SqlitePool,
    pub http: reqwest::Client,
    pub limiter: AuthLimiter,
}

async fn auth_rate_limit(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if req.uri().path().starts_with("/v1/auth/") {
        let ip = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        if !state.limiter.allow(&ip) {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({ "error": "rate_limited" })),
            )
                .into_response();
        }
    }
    next.run(req).await
}

pub fn router(state: AppState) -> Router {
    use axum::http::{HeaderValue, Method, header};
    use tower_http::cors::CorsLayer;

    // Restrictive CORS: only explicitly configured origins (F-42). Tauri
    // webviews and same-origin health checks are unaffected.
    let origins: Vec<HeaderValue> = state
        .cfg
        .allowed_origins
        .iter()
        .filter_map(|s| s.parse().ok())
        .collect();
    let cors = if origins.is_empty() {
        CorsLayer::new()
    } else {
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
    };

    Router::new()
        .route("/health", get(health))
        .route("/v1/auth/google", post(auth_google))
        .route("/v1/auth/google/url", get(auth_google_url))
        .route("/v1/auth/google/code", post(auth_google_code))
        .route("/v1/auth/dev", post(auth_dev))
        .route("/v1/auth/refresh", post(auth_refresh))
        .route("/v1/auth/logout", post(auth_logout))
        .route("/v1/me", get(me))
        .route("/v1/vault", get(get_vault).put(put_vault))
        .route("/v1/devices", get(list_devices))
        .route("/v1/devices/{id}", delete(delete_device))
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            state.cfg.max_vault_bytes.min(8 * 1024 * 1024) + 64 * 1024,
        ))
        .layer(cors)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_rate_limit,
        ))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "service": "rterm-sync", "version": env!("CARGO_PKG_VERSION") }))
}

// ---------------------------------------------------------------- auth tokens

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    email: String,
    did: String,
    typ: String,
    iat: i64,
    exp: i64,
}

fn issue_access_token(cfg: &Config, user: &UserRow, device_id: &str) -> Result<(String, u64), ApiError> {
    let now = Utc::now();
    let exp = now + Duration::seconds(cfg.access_token_ttl.as_secs() as i64);
    let claims = Claims {
        sub: user.id.clone(),
        email: user.email.clone(),
        did: device_id.to_string(),
        typ: "access".into(),
        iat: now.timestamp(),
        exp: exp.timestamp(),
    };
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(cfg.jwt_secret.as_bytes()),
    )
    .map_err(|e| ApiError::Internal(e.into()))?;
    Ok((token, cfg.access_token_ttl.as_secs()))
}

fn new_refresh_token() -> String {
    format!(
        "{}.{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DeviceIn {
    name: String,
    platform: String,
    app_version: String,
}

impl DeviceIn {
    fn normalized(&self) -> (String, String, String) {
        let name = if self.name.trim().is_empty() {
            "Unknown device".to_string()
        } else {
            self.name.trim().to_string()
        };
        let platform = if self.platform.trim().is_empty() {
            "unknown".to_string()
        } else {
            self.platform.trim().to_string()
        };
        (name, platform, self.app_version.clone())
    }
}

#[derive(Serialize)]
struct AuthResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
    user: UserRow,
}

async fn finish_login(
    state: &AppState,
    google_sub: &str,
    email: &str,
    name: Option<&str>,
    avatar_url: Option<&str>,
    device: &DeviceIn,
) -> ApiResult<AuthResponse> {
    let user = db::upsert_user(&state.db, google_sub, email, name, avatar_url).await?;
    let refresh = new_refresh_token();
    let refresh_expires_at = Utc::now() + Duration::days(state.cfg.refresh_token_ttl_days);
    let (dev_name, platform, version) = device.normalized();
    let device_id = db::create_device(
        &state.db,
        &user.id,
        &dev_name,
        &platform,
        &version,
        &hash_token(&refresh),
        refresh_expires_at,
    )
    .await?;
    let (access_token, expires_in) = issue_access_token(&state.cfg, &user, &device_id)?;
    Ok(AuthResponse {
        access_token,
        refresh_token: refresh,
        expires_in,
        user,
    })
}

#[derive(Deserialize)]
struct GoogleLoginBody {
    id_token: String,
    #[serde(default)]
    device: DeviceIn,
}

async fn auth_google(
    State(state): State<AppState>,
    Json(body): Json<GoogleLoginBody>,
) -> ApiResult<Json<AuthResponse>> {
    let (client_id, _) = state.cfg.google()?;
    let claims = google::verify_id_token(
        &state.http,
        client_id,
        &body.id_token,
        &state.cfg.google_allowed_domains,
    )
    .await?;
    let res = finish_login(
        &state,
        &claims.sub,
        &claims.email,
        claims.name.as_deref(),
        claims.picture.as_deref(),
        &body.device,
    )
    .await?;
    Ok(Json(res))
}

#[derive(Deserialize)]
struct GoogleCodeBody {
    code: String,
    #[serde(default)]
    redirect_uri: Option<String>,
    #[serde(default)]
    device: DeviceIn,
}

async fn auth_google_code(
    State(state): State<AppState>,
    Json(body): Json<GoogleCodeBody>,
) -> ApiResult<Json<AuthResponse>> {
    let (client_id, client_secret) = state.cfg.google()?;
    let redirect = body
        .redirect_uri
        .clone()
        .or_else(|| state.cfg.google_redirect_uri.clone())
        .ok_or_else(|| ApiError::BadRequest("redirect_uri is required".into()))?;
    let claims = google::exchange_code(
        &state.http,
        client_id,
        client_secret,
        &body.code,
        &redirect,
        &state.cfg.google_allowed_domains,
    )
    .await?;
    let res = finish_login(
        &state,
        &claims.sub,
        &claims.email,
        claims.name.as_deref(),
        claims.picture.as_deref(),
        &body.device,
    )
    .await?;
    Ok(Json(res))
}

#[derive(Deserialize)]
struct UrlQuery {
    redirect_uri: String,
    #[serde(default)]
    state: String,
}

async fn auth_google_url(
    State(state): State<AppState>,
    Query(q): Query<UrlQuery>,
) -> ApiResult<Json<Value>> {
    let (client_id, _) = state.cfg.google()?;
    let url = google::authorization_url(client_id, &q.redirect_uri, &q.state);
    Ok(Json(json!({ "url": url })))
}

#[derive(Deserialize)]
struct DevLoginBody {
    email: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    device: DeviceIn,
}

async fn auth_dev(
    State(state): State<AppState>,
    Json(body): Json<DevLoginBody>,
) -> ApiResult<Json<AuthResponse>> {
    if !state.cfg.dev_auth {
        return Err(ApiError::NotFound("dev auth is disabled".into()));
    }
    if !body.email.contains('@') {
        return Err(ApiError::BadRequest("invalid email".into()));
    }
    let res = finish_login(
        &state,
        &format!("dev:{}", body.email.to_lowercase()),
        &body.email,
        body.name.as_deref(),
        None,
        &body.device,
    )
    .await?;
    Ok(Json(res))
}

#[derive(Deserialize)]
struct RefreshBody {
    refresh_token: String,
}

async fn auth_refresh(
    State(state): State<AppState>,
    Json(body): Json<RefreshBody>,
) -> ApiResult<Json<AuthResponse>> {
    let hash = hash_token(&body.refresh_token);
    let device = db::find_device_by_refresh(&state.db, &hash)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("invalid refresh token".into()))?;
    let user = db::get_user(&state.db, &device.user_id)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("user no longer exists".into()))?;

    // Rotate the refresh token on every use. The conditional update rejects
    // concurrent/stale reuse of the old token (F-09/F-45).
    let new_refresh = new_refresh_token();
    let new_expires_at = Utc::now() + Duration::days(state.cfg.refresh_token_ttl_days);
    if !db::rotate_refresh(
        &state.db,
        &device.device_id,
        &hash_token(&body.refresh_token),
        &hash_token(&new_refresh),
        new_expires_at,
    )
    .await?
    {
        return Err(ApiError::Unauthorized("invalid refresh token".into()));
    }
    let (access_token, expires_in) = issue_access_token(&state.cfg, &user, &device.device_id)?;

    Ok(Json(AuthResponse {
        access_token,
        refresh_token: new_refresh,
        expires_in,
        user,
    }))
}

async fn auth_logout(auth: Authed, State(state): State<AppState>) -> ApiResult<Json<Value>> {
    db::revoke_device(&state.db, &auth.user_id, &auth.device_id).await?;
    Ok(Json(json!({ "ok": true })))
}

// ------------------------------------------------------------ auth extractor

pub struct Authed {
    pub user_id: String,
    pub device_id: String,
}

impl FromRequestParts<AppState> for Authed {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ApiError::Unauthorized("missing bearer token".into()))?;
        let token = header
            .strip_prefix("Bearer ")
            .ok_or_else(|| ApiError::Unauthorized("expected Bearer token".into()))?;

        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_required_spec_claims(&["exp", "sub"]);
        let data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(state.cfg.jwt_secret.as_bytes()),
            &validation,
        )
        .map_err(|_| ApiError::Unauthorized("invalid or expired token".into()))?;

        if data.claims.typ != "access" {
            return Err(ApiError::Unauthorized("wrong token type".into()));
        }
        // A revoked device must lose access immediately, even with a valid,
        // unexpired JWT (F-36). Also binds the token to its owner so a token
        // cannot be replayed against another user's device id.
        if !db::device_is_active(&state.db, &data.claims.sub, &data.claims.did).await? {
            return Err(ApiError::Unauthorized("device revoked".into()));
        }
        let _ = db::touch_device(&state.db, &data.claims.did).await;
        Ok(Authed {
            user_id: data.claims.sub,
            device_id: data.claims.did,
        })
    }
}

async fn me(auth: Authed, State(state): State<AppState>) -> ApiResult<Json<UserRow>> {
    let user = db::get_user(&state.db, &auth.user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("user not found".into()))?;
    Ok(Json(user))
}

// ------------------------------------------------------------------- vault

async fn get_vault(auth: Authed, State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let record = db::get_vault(&state.db, &auth.user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("no vault yet".into()))?;
    let envelope: Value = serde_json::from_str(&record.envelope)
        .map_err(|e| ApiError::Internal(e.into()))?;
    Ok(Json(json!({
        "revision": record.revision,
        "envelope": envelope,
        "updated_at": record.updated_at,
    })))
}

#[derive(Deserialize)]
struct PutVaultBody {
    revision: i64,
    envelope: Value,
}

async fn put_vault(
    auth: Authed,
    State(state): State<AppState>,
    Json(body): Json<PutVaultBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    // The server cannot inspect the payload, only size-limit it.
    let envelope = serde_json::to_string(&body.envelope)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    if envelope.len() > state.cfg.max_vault_bytes {
        return Err(ApiError::TooLarge);
    }
    if body.envelope.get("ciphertext_b64").is_none() {
        return Err(ApiError::BadRequest("not a sealed vault".into()));
    }
    let revision = db::put_vault(&state.db, &auth.user_id, body.revision, &envelope)
        .await?
        .ok_or(ApiError::Conflict)?;
    Ok((StatusCode::OK, Json(json!({ "revision": revision }))))
}

// ----------------------------------------------------------------- devices

async fn list_devices(auth: Authed, State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let rows = db::list_devices(&state.db, &auth.user_id).await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|d| {
            json!({
                "id": d.id,
                "name": d.name,
                "platform": d.platform,
                "app_version": d.app_version,
                "last_seen_at": d.last_seen_at,
                "current": d.id == auth.device_id,
            })
        })
        .collect();
    Ok(Json(json!({ "devices": items })))
}

async fn delete_device(
    auth: Authed,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    if id == auth.device_id {
        return Err(ApiError::BadRequest(
            "cannot revoke the device you are using".into(),
        ));
    }
    let ok = db::revoke_device(&state.db, &auth.user_id, &id).await?;
    if !ok {
        return Err(ApiError::NotFound("device not found".into()));
    }
    Ok(Json(json!({ "ok": true })))
}
