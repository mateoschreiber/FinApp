use crate::{AppState, db::Database, finance};
use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{
        SaltString,
        rand_core::{OsRng, RngCore},
    },
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use uuid::Uuid;

#[derive(Clone, Serialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub is_admin: bool,
}

#[derive(Serialize)]
pub struct UserSummary {
    pub id: String,
    pub username: String,
    pub is_admin: bool,
    pub active_sessions: i64,
    pub operations_count: i64,
}

#[derive(Deserialize)]
pub struct Credentials {
    username: String,
    password: String,
    #[serde(default)]
    is_admin: bool,
}

#[derive(Deserialize)]
pub struct ProfileUpdate {
    current_password: String,
    username: Option<String>,
    new_password: Option<String>,
}

#[derive(Deserialize)]
pub struct AdminUserUpdate {
    username: Option<String>,
    password: Option<String>,
    is_admin: Option<bool>,
}

fn hash_password(password: &str) -> Result<String, ()> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|s| s.to_string())
        .map_err(|_| ())
}

fn verify_password(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash).ok().is_some_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}

fn valid_username(username: &str) -> bool {
    (1..=64).contains(&username.len())
        && username.trim() == username
        && !username.chars().any(char::is_control)
}

fn valid_password(password: &str) -> bool {
    (12..=256).contains(&password.len())
}

fn valid_credentials(username: &str, password: &str) -> bool {
    valid_username(username) && valid_password(password)
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
const SESSION_TTL: i64 = 5 * 60;

fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

impl Database {
    /// Only initializes an empty database. Never changes an existing user or password.
    pub fn bootstrap_admin(&self, username: &str, password: &str) -> Result<(), String> {
        if !valid_credentials(username, password) {
            return Err("invalid bootstrap credentials".into());
        }
        let hash = hash_password(password).map_err(|_| "password hashing failed")?;
        let mut conn = self.0.lock().map_err(|_| "database unavailable")?;
        let tx = conn.transaction().map_err(|_| "database unavailable")?;
        let count: i64 = tx
            .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
            .map_err(|_| "database unavailable")?;
        if count == 0 {
            tx.execute(
                "INSERT INTO users(id, username, password_hash, is_admin) VALUES (?1, ?2, ?3, 1)",
                params![Uuid::new_v4().to_string(), username, hash],
            )
            .map_err(|_| "database unavailable")?;
        }
        tx.commit().map_err(|_| "database unavailable")?;
        Ok(())
    }
}

fn cookie_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| part.trim().strip_prefix("__Host-finapp_session="))
}

pub fn current_user(db: &Database, headers: &HeaderMap) -> Option<User> {
    let token = cookie_token(headers)?;
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let hash = token_hash(token);
    let current = now();
    let conn = db.0.lock().ok()?;
    let mut stmt = conn
        .prepare_cached("SELECT u.id,u.username,u.is_admin FROM users u JOIN sessions s ON s.user_id=u.id WHERE s.token_hash=?1 AND s.expires_at>?2")
        .ok()?;
    let user = stmt
        .query_row(params![hash, current], |r| {
            Ok(User {
                id: r.get(0)?,
                username: r.get(1)?,
                is_admin: r.get(2)?,
            })
        })
        .optional()
        .ok()
        .flatten()?;
    let _ = conn.execute(
        "UPDATE sessions SET expires_at=?1 WHERE token_hash=?2",
        params![current + SESSION_TTL, hash],
    );
    Some(user)
}

// shortcut: one global budget of 20 login attempts per 60 s, so one client can lock everyone out for up to a minute; a per-IP limit does not work behind the local proxy, which hides the real client address. Upgrade only if the proxy forwards trusted client addresses.
fn admit_login_attempt(attempts: &mut VecDeque<i64>, now: i64) -> bool {
    let cutoff = now - 60;
    while attempts.front().is_some_and(|time| *time <= cutoff) {
        attempts.pop_front();
    }
    if attempts.len() >= 20 {
        return false;
    }
    attempts.push_back(now);
    true
}

pub async fn login(State(state): State<AppState>, Json(creds): Json<Credentials>) -> Response {
    // Limit aggregate failed and successful attempts, including unknown users.
    {
        let mut attempts = match state.attempts.lock() {
            Ok(a) => a,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        if !admit_login_attempt(&mut attempts, now()) {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
    }
    if creds.username.len() > 64 || creds.password.len() > 256 {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let row: Option<(User, String)> = match state.db.0.lock() {
        Ok(conn) => match conn
            .query_row(
                "SELECT id,username,is_admin,password_hash FROM users WHERE username=?1",
                params![creds.username],
                |r| {
                    Ok((
                        User {
                            id: r.get(0)?,
                            username: r.get(1)?,
                            is_admin: r.get(2)?,
                        },
                        r.get(3)?,
                    ))
                },
            )
            .optional()
        {
            Ok(v) => v,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        },
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let Some((user, hash)) = row else {
        // Equalize the expensive part of authentication for unknown usernames without blocking Tokio workers.
        let password = creds.password;
        let _ = tokio::task::spawn_blocking(move || {
            static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            let dummy = DUMMY.get_or_init(|| {
                hash_password("not-a-real-user-password").expect("dummy password must hash")
            });
            if let Ok(parsed) = PasswordHash::new(dummy) {
                let _ = Argon2::default().verify_password(password.as_bytes(), &parsed);
            }
        })
        .await;
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let password = creds.password;
    let verified = tokio::task::spawn_blocking(move || verify_password(&hash, &password))
        .await
        .unwrap_or(false);
    if !verified {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = hex::encode(bytes);
    let token_digest = token_hash(&token);
    let user_id = user.id.clone();
    let saved = tokio::task::spawn_blocking(move || {
        let conn = state.db.0.lock().ok()?;
        // Login is the one frequent write; piggyback the purge of expired sessions here
        // so the table only grows with real active sessions.
        let current = now();
        conn.execute(
            "DELETE FROM sessions WHERE expires_at<=?1",
            params![current],
        )
        .ok()?;
        conn.execute(
            "INSERT INTO sessions(token_hash,user_id,expires_at) VALUES (?1,?2,?3)",
            params![token_digest, user_id, current + SESSION_TTL],
        )
        .ok()
    })
    .await
    .ok()
    .flatten();
    if saved.is_none() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let cookie =
        format!("__Host-finapp_session={token}; Path=/; HttpOnly; Secure; SameSite=Strict");
    ([(header::SET_COOKIE, cookie)], Json(user)).into_response()
}

pub async fn me(State(state): State<AppState>, headers: HeaderMap) -> Response {
    tokio::task::spawn_blocking(move || match current_user(&state.db, &headers) {
        Some(user) => Json(user).into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    tokio::task::spawn_blocking(move || {
        if let Err(status) = finance::bound_user(&state.db, &headers) {
            return status.into_response();
        }
        if let Some(token) = cookie_token(&headers) {
            let Ok(conn) = state.db.0.lock() else {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            };
            if conn
                .execute(
                    "DELETE FROM sessions WHERE token_hash=?1",
                    params![token_hash(token)],
                )
                .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
        (
            StatusCode::NO_CONTENT,
            [(
                header::SET_COOKIE,
                "__Host-finapp_session=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0",
            )],
        )
            .into_response()
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn admin_user(db: &Database, headers: &HeaderMap) -> Result<User, StatusCode> {
    let user = finance::bound_user(db, headers)?;
    if user.is_admin {
        Ok(user)
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

pub async fn update_me(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ProfileUpdate>,
) -> Response {
    tokio::task::spawn_blocking(move || {
        let mut user = match finance::bound_user(&state.db, &headers) {
            Ok(u) => u,
            Err(status) => return status.into_response(),
        };
        if input.current_password.len() > 256 {
            return (StatusCode::BAD_REQUEST, "La contraseña actual no coincide.").into_response();
        }
        let next_username = input
            .username
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let next_password = input.new_password.as_deref().filter(|s| !s.is_empty());
        if (next_username.is_none() && next_password.is_none())
            || !next_username.is_none_or(valid_username)
            || !next_password.is_none_or(valid_password)
        {
            return StatusCode::BAD_REQUEST.into_response();
        }
        let stored_hash: String = match state.db.0.lock() {
            Ok(conn) => match conn
                .query_row(
                    "SELECT password_hash FROM users WHERE id=?1",
                    params![user.id],
                    |r| r.get(0),
                )
                .optional()
            {
                Ok(Some(h)) => h,
                Ok(None) => return StatusCode::UNAUTHORIZED.into_response(),
                Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            },
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        if !verify_password(&stored_hash, &input.current_password) {
            return (StatusCode::BAD_REQUEST, "La contraseña actual no coincide.").into_response();
        }
        let new_hash = match next_password {
            Some(pw) => match hash_password(pw) {
                Ok(h) => Some(h),
                Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            },
            None => None,
        };
        let mut conn = match state.db.0.lock() {
            Ok(c) => c,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        let Ok(tx) = conn.transaction() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        if let Some(name) = next_username {
            match tx.execute(
                "UPDATE users SET username=?1 WHERE id=?2",
                params![name, user.id],
            ) {
                Ok(_) => user.username = name.to_owned(),
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if e.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    return (StatusCode::CONFLICT, "Ese nombre de usuario ya existe.")
                        .into_response();
                }
                Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        if let Some(hash) = new_hash {
            if tx
                .execute(
                    "UPDATE users SET password_hash=?1 WHERE id=?2",
                    params![hash, user.id],
                )
                .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            if let Some(token) = cookie_token(&headers)
                && tx
                    .execute(
                        "DELETE FROM sessions WHERE user_id=?1 AND token_hash!=?2",
                        params![user.id, token_hash(token)],
                    )
                    .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
        if tx.commit().is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        Json(user).into_response()
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub async fn list_users(State(state): State<AppState>, headers: HeaderMap) -> Response {
    tokio::task::spawn_blocking(move || {
        if let Err(status) = admin_user(&state.db, &headers) {
            return status.into_response();
        }
        let Ok(conn) = state.db.0.lock() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        let Ok(mut stmt) = conn.prepare_cached(
            "SELECT u.id, u.username, u.is_admin,
                (SELECT COUNT(*) FROM sessions s WHERE s.user_id=u.id AND s.expires_at>?1),
                (SELECT COUNT(*) FROM finance_operations o WHERE o.user_id=u.id AND o.deleted=0)
             FROM users u ORDER BY u.username COLLATE NOCASE, u.id",
        ) else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        let rows = stmt
            .query_map(params![now()], |r| {
                Ok(UserSummary {
                    id: r.get(0)?,
                    username: r.get(1)?,
                    is_admin: r.get(2)?,
                    active_sessions: r.get(3)?,
                    operations_count: r.get(4)?,
                })
            })
            .and_then(|mapped| mapped.collect::<Result<Vec<_>, _>>());
        match rows {
            Ok(users) => Json(users).into_response(),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub async fn create_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(creds): Json<Credentials>,
) -> Response {
    tokio::task::spawn_blocking(move || {
        if let Err(status) = admin_user(&state.db, &headers) {
            return status.into_response();
        }
        if !valid_credentials(&creds.username, &creds.password) {
            return StatusCode::BAD_REQUEST.into_response();
        }
        let Ok(hash) = hash_password(&creds.password) else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        let new_user = User {
            id: Uuid::new_v4().to_string(),
            username: creds.username,
            is_admin: creds.is_admin,
        };
        let Ok(conn) = state.db.0.lock() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        match conn.execute(
            "INSERT INTO users(id,username,password_hash,is_admin) VALUES (?1,?2,?3,?4)",
            params![new_user.id, new_user.username, hash, new_user.is_admin],
        ) {
            Ok(_) => (StatusCode::CREATED, Json(new_user)).into_response(),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                (StatusCode::CONFLICT, "Ese nombre de usuario ya existe.").into_response()
            }
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub async fn update_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<AdminUserUpdate>,
) -> Response {
    tokio::task::spawn_blocking(move || {
        let admin = match admin_user(&state.db, &headers) {
            Ok(u) => u,
            Err(status) => return status.into_response(),
        };
        let next_username = input
            .username
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let next_password = input.password.as_deref().filter(|s| !s.is_empty());
        if (next_username.is_none() && next_password.is_none() && input.is_admin.is_none())
            || !next_username.is_none_or(valid_username)
            || !next_password.is_none_or(valid_password)
        {
            return StatusCode::BAD_REQUEST.into_response();
        }
        let new_hash = match next_password {
            Some(pw) => match hash_password(pw) {
                Ok(h) => Some(h),
                Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            },
            None => None,
        };
        let mut conn = match state.db.0.lock() {
            Ok(c) => c,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        let Ok(tx) = conn.transaction() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        let current: Option<(String, bool)> = match tx
            .query_row(
                "SELECT username,is_admin FROM users WHERE id=?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        {
            Ok(v) => v,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        let Some((mut username, mut is_admin)) = current else {
            return StatusCode::NOT_FOUND.into_response();
        };
        if input.is_admin == Some(false) && is_admin {
            let other_admins: i64 = match tx.query_row(
                "SELECT COUNT(*) FROM users WHERE is_admin=1 AND id!=?1",
                params![id],
                |r| r.get(0),
            ) {
                Ok(n) => n,
                Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            };
            if other_admins == 0 {
                return (
                    StatusCode::BAD_REQUEST,
                    "Debe quedar al menos un administrador.",
                )
                    .into_response();
            }
        }
        if let Some(name) = next_username {
            match tx.execute(
                "UPDATE users SET username=?1 WHERE id=?2",
                params![name, id],
            ) {
                Ok(_) => username = name.to_owned(),
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if e.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    return (StatusCode::CONFLICT, "Ese nombre de usuario ya existe.")
                        .into_response();
                }
                Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        if let Some(flag) = input.is_admin {
            if tx
                .execute(
                    "UPDATE users SET is_admin=?1 WHERE id=?2",
                    params![flag, id],
                )
                .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            is_admin = flag;
        }
        if let Some(hash) = new_hash {
            if tx
                .execute(
                    "UPDATE users SET password_hash=?1 WHERE id=?2",
                    params![hash, id],
                )
                .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            let keep = (id == admin.id)
                .then(|| cookie_token(&headers).map(token_hash))
                .flatten();
            let revoked = match keep {
                Some(token) => tx.execute(
                    "DELETE FROM sessions WHERE user_id=?1 AND token_hash!=?2",
                    params![id, token],
                ),
                None => tx.execute("DELETE FROM sessions WHERE user_id=?1", params![id]),
            };
            if revoked.is_err() {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
        if tx.commit().is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        Json(User {
            id,
            username,
            is_admin,
        })
        .into_response()
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub async fn delete_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    tokio::task::spawn_blocking(move || {
        let admin = match admin_user(&state.db, &headers) {
            Ok(u) => u,
            Err(status) => return status.into_response(),
        };
        if id == admin.id {
            return (
                StatusCode::BAD_REQUEST,
                "No puedes eliminar tu propio usuario en sesión.",
            )
                .into_response();
        }
        let mut conn = match state.db.0.lock() {
            Ok(c) => c,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        let Ok(tx) = conn.transaction() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        let exists: bool = match tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id=?1)",
            params![id],
            |r| r.get(0),
        ) {
            Ok(v) => v,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        if !exists {
            return StatusCode::NOT_FOUND.into_response();
        }
        for table in [
            "reconciliations",
            "reconciliation_lines",
            "credit_allocations",
            "credit_repayments",
            "credit_purchases",
            "credit_loans",
            "credit_requests",
            "planning_receipts",
            "planning_reviews",
            "planning_budgets",
            "planning_goals",
            "planning_recurring",
            "finance_operations",
            "finance_operation_history",
            "finance_requests",
            "finance_changes",
            "import_sources",
            "catalog_details",
            "finance_categories",
            "finance_accounts",
            "sessions",
        ] {
            if tx
                .execute(
                    &format!("DELETE FROM {table} WHERE user_id=?1"),
                    params![id],
                )
                .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
        if tx
            .execute("DELETE FROM users WHERE id=?1", params![id])
            .is_err()
            || tx.commit().is_err()
        {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        StatusCode::NO_CONTENT.into_response()
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[cfg(test)]
mod tests {
    use super::admit_login_attempt;
    use std::collections::VecDeque;

    #[test]
    fn twenty_attempts_per_window_then_recovery_after_sixty_seconds() {
        let mut attempts = VecDeque::new();
        for _ in 0..20 {
            assert!(admit_login_attempt(&mut attempts, 100));
        }
        assert!(!admit_login_attempt(&mut attempts, 100));
        assert!(!admit_login_attempt(&mut attempts, 159));
        assert!(admit_login_attempt(&mut attempts, 160));
    }

    #[test]
    fn window_releases_attempts_gradually() {
        let mut attempts = VecDeque::new();
        for _ in 0..10 {
            assert!(admit_login_attempt(&mut attempts, 0));
        }
        for _ in 0..10 {
            assert!(admit_login_attempt(&mut attempts, 30));
        }
        assert!(!admit_login_attempt(&mut attempts, 59));
        for _ in 0..10 {
            assert!(admit_login_attempt(&mut attempts, 60));
        }
        assert!(!admit_login_attempt(&mut attempts, 60));
    }

    #[test]
    fn rejected_attempts_do_not_extend_the_window() {
        let mut attempts = VecDeque::new();
        for _ in 0..20 {
            assert!(admit_login_attempt(&mut attempts, 0));
        }
        for second in 1..60 {
            assert!(!admit_login_attempt(&mut attempts, second));
        }
        assert!(admit_login_attempt(&mut attempts, 60));
    }
}
