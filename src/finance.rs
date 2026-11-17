//! User-scoped financial records and versioned, idempotent writes.
use crate::{AppState, auth, db::Database};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub initial_balance: i64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub kind: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub request_id: String,
    pub id: String,
    pub expected_version: i64,
    pub deleted: bool,
    pub kind: Option<String>,
    pub account_id: Option<String>,
    pub category_id: Option<String>,
    pub card_account_id: Option<String>,
    pub amount: Option<i64>,
    pub date: Option<String>,
    pub description: Option<String>,
    pub notes: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Operation {
    pub id: String,
    pub version: i64,
    pub deleted: bool,
    pub kind: String,
    pub account_id: String,
    pub category_id: Option<String>,
    pub card_account_id: Option<String>,
    pub amount: i64,
    pub date: String,
    pub description: Option<String>,
    pub notes: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", content = "operation", rename_all = "snake_case")]
pub enum Outcome {
    Applied(Operation),
    Conflict(Option<Operation>),
}
#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub accounts: Vec<Account>,
    pub categories: Vec<Category>,
    pub operations: Vec<Operation>,
}
#[derive(Debug)]
pub enum Error {
    Invalid,
    DuplicateRequest,
    Database,
}
impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self {
        Self::Database
    }
}

pub(crate) const MIGRATION: &str = "CREATE TABLE IF NOT EXISTS finance_accounts(user_id TEXT NOT NULL,id TEXT NOT NULL,name TEXT NOT NULL,kind TEXT NOT NULL,initial_balance INTEGER NOT NULL,PRIMARY KEY(user_id,id));
      CREATE TABLE IF NOT EXISTS finance_categories(user_id TEXT NOT NULL,id TEXT NOT NULL,name TEXT NOT NULL,kind TEXT NOT NULL,PRIMARY KEY(user_id,id));
      CREATE TABLE IF NOT EXISTS finance_operations(user_id TEXT NOT NULL,id TEXT NOT NULL,version INTEGER NOT NULL,data TEXT NOT NULL,PRIMARY KEY(user_id,id));
      CREATE TABLE IF NOT EXISTS finance_requests(user_id TEXT NOT NULL,id TEXT NOT NULL,request TEXT NOT NULL,response TEXT NOT NULL,PRIMARY KEY(user_id,id));";
pub fn initialize(db: &Database) -> Result<(), Error> {
    let conn = db.0.lock().map_err(|_| Error::Database)?;
    conn.execute_batch(MIGRATION)?;
    Ok(())
}
fn id_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}
fn name_ok(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 160 && !s.chars().any(char::is_control)
}
const SAFE: i64 = 9_007_199_254_740_991;
fn validate_balances(conn: &rusqlite::Connection, user: &str) -> Result<(), Error> {
    use std::collections::HashMap;
    let mut stmt =
        conn.prepare("SELECT id,kind,initial_balance FROM finance_accounts WHERE user_id=?1")?;
    let accounts = stmt
        .query_map([user], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut amounts: HashMap<String, (String, i128)> = accounts
        .into_iter()
        .map(|(id, kind, amount)| (id, (kind, i128::from(amount))))
        .collect();
    let mut stmt = conn.prepare("SELECT data FROM finance_operations WHERE user_id=?1")?;
    let rows = stmt
        .query_map([user], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for row in rows {
        let op: Operation = serde_json::from_str(&row).map_err(|_| Error::Database)?;
        if op.deleted {
            continue;
        }
        let source = amounts.get_mut(&op.account_id).ok_or(Error::Database)?;
        let amount = i128::from(op.amount);
        match op.kind.as_str() {
            "income" => source.1 += if source.0 == "card" { -amount } else { amount },
            "expense" => source.1 += if source.0 == "card" { amount } else { -amount },
            "payment" => {
                source.1 -= amount;
                let card = amounts
                    .get_mut(op.card_account_id.as_deref().ok_or(Error::Database)?)
                    .ok_or(Error::Database)?;
                card.1 -= amount;
            }
            _ => return Err(Error::Database),
        }
    }
    if amounts
        .values()
        .any(|(_, amount)| amount.abs() > i128::from(SAFE))
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
pub fn save_account(db: &Database, user: &str, value: &Account) -> Result<(), Error> {
    if !id_ok(&value.id)
        || !name_ok(&value.name)
        || !["cash", "bank", "wallet", "card"].contains(&value.kind.as_str())
        || value.initial_balance.unsigned_abs() > SAFE as u64
    {
        return Err(Error::Invalid);
    }
    let mut conn = db.0.lock().map_err(|_| Error::Database)?;
    let tx = conn.transaction()?;
    let previous: Option<String> = tx
        .query_row(
            "SELECT kind FROM finance_accounts WHERE user_id=?1 AND id=?2",
            params![user, value.id],
            |r| r.get(0),
        )
        .optional()?;
    if previous.as_ref().is_some_and(|v| v != &value.kind) {
        return Err(Error::Invalid);
    }
    tx.execute("INSERT INTO finance_accounts VALUES(?1,?2,?3,?4,?5) ON CONFLICT(user_id,id) DO UPDATE SET name=excluded.name,initial_balance=excluded.initial_balance",params![user,value.id,value.name,value.kind,value.initial_balance])?;
    validate_balances(&tx, user)?;
    tx.commit()?;
    Ok(())
}
pub fn save_category(db: &Database, user: &str, value: &Category) -> Result<(), Error> {
    if !id_ok(&value.id)
        || !name_ok(&value.name)
        || !["income", "expense"].contains(&value.kind.as_str())
    {
        return Err(Error::Invalid);
    }
    let conn = db.0.lock().map_err(|_| Error::Database)?;
    let previous: Option<String> = conn
        .query_row(
            "SELECT kind FROM finance_categories WHERE user_id=?1 AND id=?2",
            params![user, value.id],
            |r| r.get(0),
        )
        .optional()?;
    if previous.as_ref().is_some_and(|v| v != &value.kind) {
        return Err(Error::Invalid);
    }
    conn.execute("INSERT INTO finance_categories VALUES(?1,?2,?3,?4) ON CONFLICT(user_id,id) DO UPDATE SET name=excluded.name",params![user,value.id,value.name,value.kind])?;
    Ok(())
}
fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || b.iter()
            .enumerate()
            .any(|(i, c)| i != 4 && i != 7 && !c.is_ascii_digit())
    {
        return false;
    }
    let year: u32 = s[0..4].parse().unwrap_or(0);
    let month: u32 = s[5..7].parse().unwrap_or(0);
    let day: u32 = s[8..10].parse().unwrap_or(0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    };
    year > 0 && day > 0 && day <= days
}
fn optional_text_ok(s: &Option<String>, max: usize) -> bool {
    s.as_ref()
        .is_none_or(|v| v.len() <= max && !v.chars().any(|c| c.is_control() && c != '\n'))
}
pub fn apply(db: &Database, user: &str, c: &Change) -> Result<Outcome, Error> {
    if !id_ok(&c.id)
        || !id_ok(&c.request_id)
        || c.expected_version < 0
        || !optional_text_ok(&c.description, 500)
        || !optional_text_ok(&c.notes, 4000)
    {
        return Err(Error::Invalid);
    }
    let request = serde_json::to_string(c).map_err(|_| Error::Invalid)?;
    let mut conn = db.0.lock().map_err(|_| Error::Database)?;
    let tx = conn.transaction()?;
    let prior: Option<(String, String)> = tx
        .query_row(
            "SELECT request,response FROM finance_requests WHERE user_id=?1 AND id=?2",
            params![user, c.request_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((original, response)) = prior {
        if original != request {
            return Err(Error::DuplicateRequest);
        }
        return serde_json::from_str(&response).map_err(|_| Error::Database);
    }
    let old: Option<String> = tx
        .query_row(
            "SELECT data FROM finance_operations WHERE user_id=?1 AND id=?2",
            params![user, c.id],
            |r| r.get(0),
        )
        .optional()?;
    let current: Option<Operation> = old
        .map(|s| serde_json::from_str(&s).map_err(|_| Error::Database))
        .transpose()?;
    if current.as_ref().map_or(0, |v| v.version) != c.expected_version
        || (current.is_none() && c.deleted)
    {
        return Ok(Outcome::Conflict(current));
    }
    let next = c.expected_version.checked_add(1).ok_or(Error::Invalid)?;
    let operation = if c.deleted {
        let mut o = current.ok_or(Error::Invalid)?;
        o.version = next;
        o.deleted = true;
        o
    } else {
        let kind = c.kind.clone().ok_or(Error::Invalid)?;
        let account_id = c.account_id.clone().ok_or(Error::Invalid)?;
        let amount = c.amount.ok_or(Error::Invalid)?;
        let date = c.date.clone().ok_or(Error::Invalid)?;
        if !id_ok(&account_id) || !(1..=SAFE).contains(&amount) || !valid_date(&date) {
            return Err(Error::Invalid);
        }
        let account_kind: Option<String> = tx
            .query_row(
                "SELECT kind FROM finance_accounts WHERE user_id=?1 AND id=?2",
                params![user, account_id],
                |r| r.get(0),
            )
            .optional()?;
        let account_kind = account_kind.ok_or(Error::Invalid)?;
        match kind.as_str() {
            "income" | "expense" => {
                if c.card_account_id.is_some() {
                    return Err(Error::Invalid);
                }
                let category = c.category_id.as_ref().ok_or(Error::Invalid)?;
                let category_kind: Option<String> = tx
                    .query_row(
                        "SELECT kind FROM finance_categories WHERE user_id=?1 AND id=?2",
                        params![user, category],
                        |r| r.get(0),
                    )
                    .optional()?;
                if category_kind.as_deref() != Some(kind.as_str()) {
                    return Err(Error::Invalid);
                }
            }
            "payment" => {
                let card = c.card_account_id.as_ref().ok_or(Error::Invalid)?;
                let card_kind: Option<String> = tx
                    .query_row(
                        "SELECT kind FROM finance_accounts WHERE user_id=?1 AND id=?2",
                        params![user, card],
                        |r| r.get(0),
                    )
                    .optional()?;
                if account_kind == "card"
                    || card_kind.as_deref() != Some("card")
                    || card == &account_id
                    || c.category_id.is_some()
                {
                    return Err(Error::Invalid);
                }
            }
            _ => return Err(Error::Invalid),
        }
        Operation {
            id: c.id.clone(),
            version: next,
            deleted: false,
            kind,
            account_id,
            category_id: c.category_id.clone(),
            card_account_id: c.card_account_id.clone(),
            amount,
            date,
            description: c.description.clone(),
            notes: c.notes.clone(),
        }
    };
    let data = serde_json::to_string(&operation).map_err(|_| Error::Database)?;
    tx.execute("INSERT INTO finance_operations VALUES(?1,?2,?3,?4) ON CONFLICT(user_id,id) DO UPDATE SET version=excluded.version,data=excluded.data",params![user,c.id,next,data])?;
    validate_balances(&tx, user)?;
    let outcome = Outcome::Applied(operation);
    let response = serde_json::to_string(&outcome).map_err(|_| Error::Database)?;
    tx.execute(
        "INSERT INTO finance_requests VALUES(?1,?2,?3,?4)",
        params![user, c.request_id, request, response],
    )?;
    tx.commit()?;
    Ok(outcome)
}
pub fn snapshot(db: &Database, user: &str) -> Result<Snapshot, Error> {
    let conn = db.0.lock().map_err(|_| Error::Database)?;
    let mut stmt=conn.prepare("SELECT id,name,kind,initial_balance FROM finance_accounts WHERE user_id=?1 ORDER BY name,id")?;
    let accounts = stmt
        .query_map([user], |r| {
            Ok(Account {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
                initial_balance: r.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut stmt = conn
        .prepare("SELECT id,name,kind FROM finance_categories WHERE user_id=?1 ORDER BY name,id")?;
    let categories = stmt
        .query_map([user], |r| {
            Ok(Category {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut stmt =
        conn.prepare("SELECT data FROM finance_operations WHERE user_id=?1 ORDER BY id")?;
    let rows = stmt
        .query_map([user], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let operations = rows
        .into_iter()
        .map(|s| serde_json::from_str(&s).map_err(|_| Error::Database))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Snapshot {
        accounts,
        categories,
        operations,
    })
}
fn fail(error: Error) -> Response {
    match error {
        Error::Invalid => StatusCode::BAD_REQUEST,
        Error::DuplicateRequest => StatusCode::CONFLICT,
        Error::Database => StatusCode::INTERNAL_SERVER_ERROR,
    }
    .into_response()
}
fn bound_user(db: &Database, h: &HeaderMap) -> Result<auth::User, StatusCode> {
    let user = auth::current_user(db, h).ok_or(StatusCode::UNAUTHORIZED)?;
    if h.get("x-finapp-user").and_then(|v| v.to_str().ok()) != Some(user.id.as_str()) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(user)
}
async fn get_snapshot(State(s): State<AppState>, h: HeaderMap) -> Response {
    let u = match bound_user(&s.db, &h) {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    match snapshot(&s.db, &u.id) {
        Ok(v) => Json(v).into_response(),
        Err(e) => fail(e),
    }
}
async fn post_account(State(s): State<AppState>, h: HeaderMap, Json(v): Json<Account>) -> Response {
    let u = match bound_user(&s.db, &h) {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    match save_account(&s.db, &u.id, &v) {
        Ok(()) => Json(v).into_response(),
        Err(e) => fail(e),
    }
}
async fn post_category(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(v): Json<Category>,
) -> Response {
    let u = match bound_user(&s.db, &h) {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    match save_category(&s.db, &u.id, &v) {
        Ok(()) => Json(v).into_response(),
        Err(e) => fail(e),
    }
}
async fn post_operation(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(v): Json<Change>,
) -> Response {
    let u = match bound_user(&s.db, &h) {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    match apply(&s.db, &u.id, &v) {
        Ok(result) => {
            let status = if matches!(result, Outcome::Applied(_)) {
                StatusCode::OK
            } else {
                StatusCode::CONFLICT
            };
            (status, Json(result)).into_response()
        }
        Err(e) => fail(e),
    }
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/snapshot", get(get_snapshot))
        .route("/api/accounts", post(post_account))
        .route("/api/categories", post(post_category))
        .route("/api/operations", post(post_operation))
}
