use crate::{AppState, db::Database};
use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use rand::RngCore;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Clone, Serialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub is_admin: bool,
}

#[derive(Deserialize)]
pub struct Credentials {
    username: String,
    password: String,
}

fn hash_password(password: &str) -> Result<String, ()> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|s| s.to_string())
        .map_err(|_| ())
}

fn valid_credentials(username: &str, password: &str) -> bool {
    (1..=64).contains(&username.len())
        && username.trim() == username
        && !username.chars().any(char::is_control)
        && (12..=256).contains(&password.len())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
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
    let conn = db.0.lock().ok()?;
    conn.query_row("SELECT u.id,u.username,u.is_admin FROM users u JOIN sessions s ON s.user_id=u.id WHERE s.token_hash=?1 AND s.expires_at>?2", params![token_hash(token), now()], |r| Ok(User { id: r.get(0)?, username: r.get(1)?, is_admin: r.get(2)? })).optional().ok().flatten()
}

pub async fn login(State(state): State<AppState>, Json(creds): Json<Credentials>) -> Response {
    // Limit aggregate failed and successful attempts, including unknown users.
    {
        let mut attempts = match state.attempts.lock() {
            Ok(a) => a,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        let cutoff = now() - 60;
        while attempts.front().is_some_and(|time| *time <= cutoff) {
            attempts.pop_front();
        }
        if attempts.len() >= 20 {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
        attempts.push_back(now());
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
        // Equalize the expensive part of authentication for unknown usernames.
        static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        let dummy = DUMMY.get_or_init(|| {
            hash_password("not-a-real-user-password").expect("dummy password must hash")
        });
        if let Ok(parsed) = PasswordHash::new(dummy) {
            let _ = Argon2::default().verify_password(creds.password.as_bytes(), &parsed);
        }
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !PasswordHash::new(&hash).ok().is_some_and(|parsed| {
        Argon2::default()
            .verify_password(creds.password.as_bytes(), &parsed)
            .is_ok()
    }) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token = hex::encode(bytes);
    let conn = match state.db.0.lock() {
        Ok(c) => c,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    // Login is the one frequent write; piggyback the purge of expired sessions here
    // so the table only grows with real active sessions.
    if conn
        .execute("DELETE FROM sessions WHERE expires_at<=?1", params![now()])
        .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if conn
        .execute(
            "INSERT INTO sessions(token_hash,user_id,expires_at) VALUES (?1,?2,?3)",
            params![token_hash(&token), user.id, now() + 86400 * 30],
        )
        .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let cookie = format!(
        "__Host-finapp_session={token}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=2592000"
    );
    ([(header::SET_COOKIE, cookie)], Json(user)).into_response()
}

pub async fn me(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match current_user(&state.db, &headers) {
        Some(user) => Json(user).into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(user) = current_user(&state.db, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if headers
        .get("x-finapp-user")
        .and_then(|value| value.to_str().ok())
        != Some(user.id.as_str())
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(token) = cookie_token(&headers) {
        match state.db.0.lock() {
            Ok(conn) => {
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
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
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
}

pub async fn create_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(creds): Json<Credentials>,
) -> Response {
    let Some(user) = current_user(&state.db, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if headers
        .get("x-finapp-user")
        .and_then(|value| value.to_str().ok())
        != Some(user.id.as_str())
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !user.is_admin {
        return StatusCode::FORBIDDEN.into_response();
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
        is_admin: false,
    };
    let conn = match state.db.0.lock() {
        Ok(c) => c,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    match conn.execute(
        "INSERT INTO users(id,username,password_hash,is_admin) VALUES (?1,?2,?3,0)",
        params![new_user.id, new_user.username, hash],
    ) {
        Ok(_) => (StatusCode::CREATED, Json(new_user)).into_response(),
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            StatusCode::CONFLICT.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
