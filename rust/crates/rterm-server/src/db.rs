//! SQLite persistence.
//!
//! Only opaque data lives here: accounts, devices and the *encrypted* vault
//! envelope. SSH passwords/keys are inside that envelope, so the database (and
//! its backups) never contain a readable secret.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub struct UserRow {
    pub id: String,
    pub email: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeviceRow {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub app_version: String,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub revoked: bool,
}

pub async fn connect(database_url: &str) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(database_url)
        .with_context(|| format!("invalid DATABASE_URL: {database_url}"))?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(5));

    if let Some(path) = options.get_filename().to_str() {
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await
        .context("cannot open sqlite database")?;
    init_schema(&pool).await?;
    Ok(pool)
}

async fn init_schema(pool: &SqlitePool) -> Result<()> {
    let statements = [
        r#"CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            google_sub TEXT NOT NULL UNIQUE,
            email TEXT NOT NULL,
            name TEXT,
            avatar_url TEXT,
            created_at TEXT NOT NULL,
            last_login_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS devices (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            platform TEXT NOT NULL,
            app_version TEXT NOT NULL DEFAULT '',
            refresh_hash TEXT,
            created_at TEXT NOT NULL,
            last_seen_at TEXT,
            revoked_at TEXT
        )"#,
        "CREATE INDEX IF NOT EXISTS idx_devices_user ON devices(user_id)",
        "CREATE INDEX IF NOT EXISTS idx_devices_refresh ON devices(refresh_hash)",
        r#"CREATE TABLE IF NOT EXISTS vaults (
            user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
            revision INTEGER NOT NULL,
            envelope TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )"#,
    ];
    for stmt in statements {
        sqlx::query(stmt).execute(pool).await?;
    }
    Ok(())
}

pub async fn upsert_user(
    pool: &SqlitePool,
    google_sub: &str,
    email: &str,
    name: Option<&str>,
    avatar_url: Option<&str>,
) -> Result<UserRow> {
    let now = Utc::now();
    if let Some(row) = sqlx::query(
        "SELECT id, email, name, avatar_url FROM users WHERE google_sub = ?",
    )
    .bind(google_sub)
    .fetch_optional(pool)
    .await?
    {
        let id: String = row.get("id");
        sqlx::query("UPDATE users SET email = ?, name = ?, avatar_url = ?, last_login_at = ? WHERE id = ?")
            .bind(email)
            .bind(name)
            .bind(avatar_url)
            .bind(now)
            .bind(&id)
            .execute(pool)
            .await?;
        return Ok(UserRow {
            id,
            email: email.to_string(),
            name: name.map(str::to_string),
            avatar_url: avatar_url.map(str::to_string),
        });
    }

    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO users (id, google_sub, email, name, avatar_url, created_at, last_login_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(google_sub)
    .bind(email)
    .bind(name)
    .bind(avatar_url)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(UserRow {
        id,
        email: email.to_string(),
        name: name.map(str::to_string),
        avatar_url: avatar_url.map(str::to_string),
    })
}

pub async fn get_user(pool: &SqlitePool, id: &str) -> Result<Option<UserRow>> {
    let row = sqlx::query("SELECT id, email, name, avatar_url FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| UserRow {
        id: r.get("id"),
        email: r.get("email"),
        name: r.get("name"),
        avatar_url: r.get("avatar_url"),
    }))
}

pub async fn create_device(
    pool: &SqlitePool,
    user_id: &str,
    name: &str,
    platform: &str,
    app_version: &str,
    refresh_hash: &str,
) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO devices (id, user_id, name, platform, app_version, refresh_hash, created_at, last_seen_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(user_id)
    .bind(name)
    .bind(platform)
    .bind(app_version)
    .bind(refresh_hash)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(id)
}

pub struct RefreshDevice {
    pub device_id: String,
    pub user_id: String,
}

pub async fn find_device_by_refresh(
    pool: &SqlitePool,
    refresh_hash: &str,
) -> Result<Option<RefreshDevice>> {
    let row = sqlx::query(
        "SELECT id, user_id FROM devices WHERE refresh_hash = ? AND revoked_at IS NULL",
    )
    .bind(refresh_hash)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| RefreshDevice {
        device_id: r.get("id"),
        user_id: r.get("user_id"),
    }))
}

pub async fn rotate_refresh(
    pool: &SqlitePool,
    device_id: &str,
    new_hash: &str,
) -> Result<()> {
    sqlx::query("UPDATE devices SET refresh_hash = ?, last_seen_at = ? WHERE id = ?")
        .bind(new_hash)
        .bind(Utc::now())
        .bind(device_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn touch_device(pool: &SqlitePool, device_id: &str) -> Result<()> {
    sqlx::query("UPDATE devices SET last_seen_at = ? WHERE id = ?")
        .bind(Utc::now())
        .bind(device_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn list_devices(pool: &SqlitePool, user_id: &str) -> Result<Vec<DeviceRow>> {
    let rows = sqlx::query(
        "SELECT id, name, platform, app_version, last_seen_at, revoked_at
         FROM devices WHERE user_id = ? ORDER BY last_seen_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| DeviceRow {
            id: r.get("id"),
            name: r.get("name"),
            platform: r.get("platform"),
            app_version: r.get("app_version"),
            last_seen_at: r.get("last_seen_at"),
            revoked: r.get::<Option<DateTime<Utc>>, _>("revoked_at").is_some(),
        })
        .collect())
}

pub async fn revoke_device(pool: &SqlitePool, user_id: &str, device_id: &str) -> Result<bool> {
    let res = sqlx::query(
        "UPDATE devices SET revoked_at = ?, refresh_hash = NULL WHERE id = ? AND user_id = ?",
    )
    .bind(Utc::now())
    .bind(device_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub struct VaultRecord {
    pub revision: i64,
    pub envelope: String,
    pub updated_at: DateTime<Utc>,
}

pub async fn get_vault(pool: &SqlitePool, user_id: &str) -> Result<Option<VaultRecord>> {
    let row = sqlx::query("SELECT revision, envelope, updated_at FROM vaults WHERE user_id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| VaultRecord {
        revision: r.get("revision"),
        envelope: r.get("envelope"),
        updated_at: r.get("updated_at"),
    }))
}

/// Compare-and-swap write. `Ok(None)` means the client revision was stale.
pub async fn put_vault(
    pool: &SqlitePool,
    user_id: &str,
    base_revision: i64,
    envelope: &str,
) -> Result<Option<i64>> {
    let now = Utc::now();
    let mut tx = pool.begin().await?;
    let current: Option<i64> =
        sqlx::query_scalar("SELECT revision FROM vaults WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;

    let new_revision = match current {
        None => {
            if base_revision != 0 {
                return Ok(None);
            }
            sqlx::query(
                "INSERT INTO vaults (user_id, revision, envelope, updated_at) VALUES (?, 1, ?, ?)",
            )
            .bind(user_id)
            .bind(envelope)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            1
        }
        Some(rev) => {
            if rev != base_revision {
                return Ok(None);
            }
            let next = rev + 1;
            sqlx::query(
                "UPDATE vaults SET revision = ?, envelope = ?, updated_at = ? WHERE user_id = ?",
            )
            .bind(next)
            .bind(envelope)
            .bind(now)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
            next
        }
    };
    tx.commit().await?;
    Ok(Some(new_revision))
}
