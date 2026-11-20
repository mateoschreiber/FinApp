//! User-scoped financial records and versioned, idempotent writes.
use crate::{AppState, auth, catalog, db::Database};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_operation_id: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_operation_id: Option<String>,
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
    pub catalog_details: Vec<crate::catalog::Detail>,
    pub cursor: i64,
    pub epoch: String,
    pub incremental: bool,
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

pub fn initialize(db: &Database) -> Result<(), Error> {
    let conn = db.0.lock().map_err(|_| Error::Database)?;
    conn.execute_batch(include_str!("../migrations/002_finance.sql"))?;
    Ok(())
}
fn id_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}
fn name_ok(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 160 && !s.chars().any(char::is_control)
}
pub(crate) const SAFE: i64 = 9_007_199_254_740_991;
pub(crate) fn validate_balance(conn: &Connection, user: &str, account: &str) -> Result<(), Error> {
    let initial: i64 = conn.query_row(
        "SELECT initial_balance FROM finance_accounts WHERE user_id=?1 AND id=?2",
        params![user, account],
        |r| r.get(0),
    )?;
    let mut stmt = conn
        .prepare("SELECT delta FROM finance_balance_postings WHERE user_id=?1 AND account_id=?2")?;
    let mut balance = i128::from(initial);
    for row in stmt.query_map(params![user, account], |r| r.get::<_, i64>(0))? {
        balance += i128::from(row?);
    }
    if balance.abs() > i128::from(SAFE) {
        return Err(Error::Invalid);
    }
    Ok(())
}

pub(crate) fn balances_in(
    conn: &Connection,
    user: &str,
    end: Option<&str>,
) -> Result<BTreeMap<String, i128>, rusqlite::Error> {
    let mut accounts = conn.prepare(
        "SELECT a.id,a.initial_balance,
        CASE WHEN ?2 IS NULL OR d.cutoff_date IS NULL OR d.cutoff_date<=?2 THEN 1 ELSE 0 END
        FROM finance_accounts a LEFT JOIN catalog_details d
        ON d.user_id=a.user_id AND d.entity='account' AND d.id=a.id WHERE a.user_id=?1",
    )?;
    let mut balances = accounts
        .query_map(params![user, end], |row| {
            let included: bool = row.get(2)?;
            let initial: i64 = row.get(1)?;
            Ok((
                row.get::<_, String>(0)?,
                if included { i128::from(initial) } else { 0 },
            ))
        })?
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let mut postings = conn.prepare(
        "SELECT account_id,delta FROM finance_balance_postings
        WHERE user_id=?1 AND (?2 IS NULL OR date<=?2)",
    )?;
    for row in postings.query_map(params![user, end], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })? {
        let (id, delta) = row?;
        *balances.get_mut(&id).ok_or(rusqlite::Error::InvalidQuery)? += i128::from(delta);
    }
    Ok(balances)
}

pub(crate) fn changed(conn: &Connection, user: &str, entity: &str, id: &str) -> Result<(), Error> {
    conn.execute(
        "INSERT OR REPLACE INTO finance_changes(user_id,entity,id) VALUES(?1,?2,?3)",
        params![user, entity, id],
    )?;
    Ok(())
}
pub(crate) fn write<T>(
    db: &Database,
    action: impl FnOnce(&Connection) -> Result<T, Error>,
) -> Result<T, Error> {
    let mut conn = db.0.lock().map_err(|_| Error::Database)?;
    let tx = conn.transaction()?;
    let result = action(&tx)?;
    tx.commit()?;
    Ok(result)
}
pub fn save_account(db: &Database, user: &str, value: &Account) -> Result<(), Error> {
    write(db, |tx| save_account_in(tx, user, value))
}
pub(crate) fn save_account_in(tx: &Connection, user: &str, value: &Account) -> Result<(), Error> {
    upsert_account_in(tx, user, value)?;
    validate_balance(tx, user, &value.id)?;
    changed(tx, user, "account", &value.id)
}
pub(crate) fn upsert_account_in(tx: &Connection, user: &str, value: &Account) -> Result<(), Error> {
    if !id_ok(&value.id)
        || !name_ok(&value.name)
        || !["cash", "bank", "wallet", "card"].contains(&value.kind.as_str())
        || value.initial_balance.unsigned_abs() > SAFE as u64
    {
        return Err(Error::Invalid);
    }
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
    Ok(())
}
pub fn save_category(db: &Database, user: &str, value: &Category) -> Result<(), Error> {
    write(db, |tx| save_category_in(tx, user, value))
}
pub(crate) fn save_category_in(
    conn: &rusqlite::Connection,
    user: &str,
    value: &Category,
) -> Result<(), Error> {
    if !id_ok(&value.id)
        || !name_ok(&value.name)
        || !["income", "expense"].contains(&value.kind.as_str())
    {
        return Err(Error::Invalid);
    }
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
    changed(conn, user, "category", &value.id)
}
pub(crate) fn valid_date(s: &str) -> bool {
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
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
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
    if c.id.starts_with("loan:")
        || matches!(c.kind.as_deref(), Some("loan_draw" | "loan_repayment"))
    {
        return Err(Error::Invalid);
    }
    write(db, |tx| apply_in(tx, user, c))
}
pub(crate) fn apply_in(
    tx: &rusqlite::Connection,
    user: &str,
    c: &Change,
) -> Result<Outcome, Error> {
    apply_change(tx, user, c, false)
}
pub(crate) fn apply_import_in(
    tx: &Connection,
    user: &str,
    change: &Change,
) -> Result<Outcome, Error> {
    if change.id.starts_with("loan:") {
        return Err(Error::Invalid);
    }
    apply_change(tx, user, change, true)
}

fn apply_change(
    tx: &Connection,
    user: &str,
    c: &Change,
    importing: bool,
) -> Result<Outcome, Error> {
    if !id_ok(&c.id)
        || !id_ok(&c.request_id)
        || c.expected_version < 0
        || !optional_text_ok(&c.description, 500)
        || !optional_text_ok(&c.notes, 4000)
    {
        return Err(Error::Invalid);
    }
    let request = serde_json::to_string(c).map_err(|_| Error::Invalid)?;
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
        let mut o = current.clone().ok_or(Error::Invalid)?;
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
        if (c.destination_account_id.is_some() && kind != "transfer")
            || (c.original_operation_id.is_some() && kind != "refund")
        {
            return Err(Error::Invalid);
        }
        match kind.as_str() {
            "transfer" => {
                let dest = c.destination_account_id.as_ref().ok_or(Error::Invalid)?;
                let eligible: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM finance_accounts WHERE user_id=?1 AND id=?2 AND kind!='card')",params![user,dest], |r| r.get(0))?;
                if !eligible
                    || dest == &account_id
                    || account_kind == "card"
                    || c.category_id.is_some()
                    || c.card_account_id.is_some()
                {
                    return Err(Error::Invalid);
                }
            }
            "refund" => {
                if c.original_operation_id.is_none()
                    || c.category_id.is_none()
                    || c.card_account_id.is_some()
                {
                    return Err(Error::Invalid);
                }
            }
            "loan_draw" | "loan_repayment" => {
                if (!importing && !c.id.starts_with("loan:"))
                    || account_kind == "card"
                    || c.category_id.is_some()
                    || c.card_account_id.is_some()
                {
                    return Err(Error::Invalid);
                }
            }
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
            destination_account_id: c.destination_account_id.clone(),
            original_operation_id: c.original_operation_id.clone(),
            amount,
            date,
            description: c.description.clone(),
            notes: c.notes.clone(),
        }
    };
    let op = &operation;
    if op.kind == "refund" && !op.deleted {
        let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM finance_operations WHERE user_id=?1 AND id=?2 AND kind='expense' AND deleted=0)",params![user,op.original_operation_id], |r|r.get(0))?;
        if !exists {
            return Err(Error::Invalid);
        }
    }
    if !importing && !op.deleted {
        for (entity, id, previous) in [
            (
                "account",
                Some(op.account_id.as_str()),
                current.as_ref().map(|o| o.account_id.as_str()),
            ),
            (
                "account",
                op.card_account_id.as_deref(),
                current.as_ref().and_then(|o| o.card_account_id.as_deref()),
            ),
            (
                "account",
                op.destination_account_id.as_deref(),
                current
                    .as_ref()
                    .and_then(|o| o.destination_account_id.as_deref()),
            ),
            (
                "category",
                op.category_id.as_deref(),
                current.as_ref().and_then(|o| o.category_id.as_deref()),
            ),
        ] {
            if let Some(id) = id.filter(|_| entity != "category" || op.kind != "refund")
                && Some(id) != previous
                && catalog::is_archived(tx, user, entity, id)?
            {
                return Err(Error::Invalid);
            }
        }
    }
    tx.execute("INSERT INTO finance_operations(user_id,id,version,deleted,kind,account_id,category_id,card_account_id,destination_account_id,original_operation_id,amount,date,description,notes)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
        ON CONFLICT(user_id,id) DO UPDATE SET version=excluded.version,deleted=excluded.deleted,kind=excluded.kind,account_id=excluded.account_id,
        category_id=excluded.category_id,card_account_id=excluded.card_account_id,destination_account_id=excluded.destination_account_id,
        original_operation_id=excluded.original_operation_id,amount=excluded.amount,date=excluded.date,description=excluded.description,notes=excluded.notes",
        params![user,op.id,op.version,op.deleted,op.kind,op.account_id,op.category_id,op.card_account_id,op.destination_account_id,op.original_operation_id,op.amount,op.date,op.description,op.notes])?;
    let invalid_refunds: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM finance_operations r JOIN finance_operations e ON e.user_id=r.user_id AND e.id=r.original_operation_id
        WHERE r.user_id=?1 AND r.deleted=0 AND (e.id=?2 OR e.id=?3) GROUP BY e.id
        HAVING SUM(r.amount)>e.amount OR MAX(e.deleted OR e.kind!='expense' OR r.date<e.date OR
            (r.account_id!=e.account_id AND EXISTS(SELECT 1 FROM finance_accounts a WHERE a.user_id=r.user_id AND a.id=r.account_id AND a.kind='card'))
            OR r.category_id IS NOT e.category_id))",
        params![user,op.id,op.original_operation_id], |r|r.get(0))?;
    if invalid_refunds {
        return Err(Error::Invalid);
    }
    let mut touched = std::collections::BTreeSet::new();
    for row in current.iter().chain(std::iter::once(op)) {
        touched.insert(&row.account_id);
        touched.extend(row.card_account_id.iter());
        touched.extend(row.destination_account_id.iter());
    }
    if !importing {
        for id in touched {
            validate_balance(tx, user, id)?;
        }
    }
    changed(tx, user, "operation", &op.id)?;
    let outcome = Outcome::Applied(operation);
    let response = serde_json::to_string(&outcome).map_err(|_| Error::Database)?;
    tx.execute(
        "INSERT INTO finance_requests VALUES(?1,?2,?3,?4)",
        params![user, c.request_id, request, response],
    )?;
    Ok(outcome)
}
pub fn snapshot(db: &Database, user: &str) -> Result<Snapshot, Error> {
    snapshot_since(db, user, None, None)
}
pub fn snapshot_since(
    db: &Database,
    user: &str,
    since: Option<i64>,
    epoch: Option<&str>,
) -> Result<Snapshot, Error> {
    let conn = db.0.lock().map_err(|_| Error::Database)?;
    let current_epoch: String = conn.query_row("SELECT id FROM sync_epoch", [], |r| r.get(0))?;
    let cursor = conn.query_row(
        "SELECT COALESCE(MAX(seq),0) FROM finance_changes WHERE user_id=?1",
        [user],
        |r| r.get::<_, i64>(0),
    )?;
    let after = since.filter(|n| (0..=cursor).contains(n) && epoch == Some(&current_epoch));
    fn selection(
        table: &str,
        entity: &str,
        fields: &str,
        order: &str,
        incremental: bool,
    ) -> String {
        let filter = if incremental {
            format!(
                " AND id IN (SELECT id FROM finance_changes WHERE user_id=?1 AND entity='{entity}' AND seq>?2)"
            )
        } else {
            " AND ?2 IS NULL".into()
        };
        format!("SELECT {fields} FROM {table} WHERE user_id=?1 {filter} ORDER BY {order}")
    }
    let mut stmt = conn.prepare(&selection(
        "finance_accounts",
        "account",
        "id,name,kind,initial_balance",
        "name,id",
        after.is_some(),
    ))?;
    let accounts = stmt
        .query_map(params![user, after], |r| {
            Ok(Account {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
                initial_balance: r.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut stmt = conn.prepare(&selection(
        "finance_categories",
        "category",
        "id,name,kind",
        "name,id",
        after.is_some(),
    ))?;
    let categories = stmt
        .query_map(params![user, after], |r| {
            Ok(Category {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut stmt = conn.prepare(&selection(
        "finance_operations",
        "operation",
        "data",
        "id",
        after.is_some(),
    ))?;
    let rows = stmt
        .query_map(params![user, after], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let operations = rows
        .into_iter()
        .map(|s| serde_json::from_str(&s).map_err(|_| Error::Database))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Snapshot {
        catalog_details: crate::catalog::list_in(&conn, user, after)?,
        cursor,
        epoch: current_epoch,
        incremental: after.is_some(),
        accounts,
        categories,
        operations,
    })
}
fn fail(error: Error) -> Response {
    status(error).into_response()
}
pub(crate) fn status(error: Error) -> StatusCode {
    match error {
        Error::Invalid => StatusCode::BAD_REQUEST,
        Error::DuplicateRequest => StatusCode::CONFLICT,
        Error::Database => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
pub(crate) fn bound_user(db: &Database, h: &HeaderMap) -> Result<auth::User, StatusCode> {
    let user = auth::current_user(db, h).ok_or(StatusCode::UNAUTHORIZED)?;
    if h.get("x-finapp-user").and_then(|v| v.to_str().ok()) != Some(user.id.as_str()) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(user)
}
#[derive(Deserialize)]
struct SnapshotQuery {
    since: Option<i64>,
    epoch: Option<String>,
}
async fn get_snapshot(
    State(s): State<AppState>,
    Query(query): Query<SnapshotQuery>,
    h: HeaderMap,
) -> Response {
    let u = match bound_user(&s.db, &h) {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    match snapshot_since(&s.db, &u.id, query.since, query.epoch.as_deref()) {
        Ok(v) => Json(v).into_response(),
        Err(e) => fail(e),
    }
}
async fn post_account(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(mut input): Json<catalog::AccountInput>,
) -> Response {
    let u = match bound_user(&s.db, &h) {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    match write(&s.db, |tx| catalog::save_account(tx, &u.id, &mut input)) {
        Ok(()) => Json(input.account).into_response(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn setup() -> Database {
        let db = Database::open_memory().unwrap();
        db.0.lock()
            .unwrap()
            .execute("INSERT INTO users VALUES('u','u','-',0)", [])
            .unwrap();
        for (id, kind) in [("cash", "cash"), ("wallet", "wallet"), ("visa", "card")] {
            save_account(
                &db,
                "u",
                &Account {
                    id: id.into(),
                    name: id.into(),
                    kind: kind.into(),
                    initial_balance: 0,
                },
            )
            .unwrap();
        }
        for id in ["food", "other"] {
            save_category(
                &db,
                "u",
                &Category {
                    id: id.into(),
                    name: id.into(),
                    kind: "expense".into(),
                },
            )
            .unwrap();
        }
        db
    }

    fn apply_json(
        db: &Database,
        request: &str,
        id: &str,
        version: i64,
        mut extra: Value,
    ) -> Result<Outcome, Error> {
        let mut value = json!({
            "request_id": request, "id": id, "expected_version": version, "deleted": false,
            "account_id": "cash", "date": "2026-10-10",
        });
        if let Some(map) = extra.as_object_mut() {
            for (key, item) in map {
                value[key] = item.take();
            }
        }
        let change: Change = serde_json::from_value(value).unwrap();
        apply(db, "u", &change)
    }

    #[test]
    fn refunds_are_capped_by_their_expense_and_follow_its_account_rules() {
        let db = setup();
        apply_json(
            &db,
            "e",
            "expense",
            0,
            json!({"kind":"expense","category_id":"food","amount":500}),
        )
        .unwrap();

        // Partial refund on the expense account applies.
        apply_json(
            &db,
            "r1",
            "refund-1",
            0,
            json!({"kind":"refund","category_id":"food","amount":200,"date":"2026-10-15","original_operation_id":"expense"}),
        )
        .unwrap();
        // Accumulated refunds may reach the original amount exactly.
        apply_json(
            &db,
            "r2",
            "refund-2",
            0,
            json!({"kind":"refund","category_id":"food","amount":300,"date":"2026-10-16","original_operation_id":"expense"}),
        )
        .unwrap();
        // One guaraní beyond the original is rejected.
        assert!(matches!(
            apply_json(
                &db,
                "r3",
                "refund-3",
                0,
                json!({"kind":"refund","category_id":"food","amount":1,"date":"2026-10-17","original_operation_id":"expense"}),
            )
            .unwrap_err(),
            Error::Invalid
        ));
        // A refund dated before the expense is rejected.
        assert!(matches!(
            apply_json(
                &db,
                "r4",
                "refund-4",
                0,
                json!({"kind":"refund","category_id":"food","amount":10,"date":"2026-10-09","original_operation_id":"expense"}),
            )
            .unwrap_err(),
            Error::Invalid
        ));
        // The expense cannot shrink below its refunds or be deleted while they exist.
        assert!(matches!(
            apply_json(
                &db,
                "e2",
                "expense",
                1,
                json!({"kind":"expense","category_id":"food","amount":100}),
            )
            .unwrap_err(),
            Error::Invalid
        ));
        let mut delete = json!({"kind":"expense","category_id":"food","amount":500});
        delete["deleted"] = Value::Bool(true);
        assert!(matches!(
            apply_json(&db, "e3", "expense", 1, delete).unwrap_err(),
            Error::Invalid
        ));
        // A refund on a different card account is rejected.
        assert!(matches!(
            apply_json(
                &db,
                "r5",
                "refund-5",
                0,
                json!({"kind":"refund","category_id":"food","amount":10,"date":"2026-10-18","original_operation_id":"expense","account_id":"visa"}),
            )
            .unwrap_err(),
            Error::Invalid
        ));
    }

    #[test]
    fn refunds_require_a_live_matching_expense_and_safe_amounts() {
        let db = setup();
        apply_json(
            &db,
            "e",
            "expense",
            0,
            json!({"kind":"expense","category_id":"food","amount":500}),
        )
        .unwrap();
        apply_json(
            &db,
            "r",
            "refund",
            0,
            json!({"kind":"refund","category_id":"food","amount":100,"date":"2026-10-15","original_operation_id":"expense"}),
        )
        .unwrap();

        // Only an expense can be refunded, never another refund.
        assert!(matches!(
            apply_json(
                &db,
                "r2",
                "refund-2",
                0,
                json!({"kind":"refund","category_id":"food","amount":10,"date":"2026-10-16","original_operation_id":"refund"}),
            )
            .unwrap_err(),
            Error::Invalid
        ));
        // The refund category must be the expense category.
        assert!(matches!(
            apply_json(
                &db,
                "r3",
                "refund-3",
                0,
                json!({"kind":"refund","category_id":"other","amount":10,"date":"2026-10-16","original_operation_id":"expense"}),
            )
            .unwrap_err(),
            Error::Invalid
        ));
        // Refunds never carry a card of their own.
        assert!(matches!(
            apply_json(
                &db,
                "r4",
                "refund-4",
                0,
                json!({"kind":"refund","category_id":"food","card_account_id":"visa","amount":10,"date":"2026-10-16","original_operation_id":"expense"}),
            )
            .unwrap_err(),
            Error::Invalid
        ));
        // Amounts beyond the safe integer range are rejected.
        assert!(matches!(
            apply_json(
                &db,
                "r5",
                "refund-5",
                0,
                json!({"kind":"refund","category_id":"food","amount":9_007_199_254_740_992_i64,"date":"2026-10-16","original_operation_id":"expense"}),
            )
            .unwrap_err(),
            Error::Invalid
        ));
        // A missing original is rejected.
        assert!(matches!(
            apply_json(
                &db,
                "r6",
                "refund-6",
                0,
                json!({"kind":"refund","category_id":"food","amount":10,"date":"2026-10-16","original_operation_id":"missing"}),
            )
            .unwrap_err(),
            Error::Invalid
        ));
        // A refund to another non-card account keeps the permissive current rule.
        apply_json(
            &db,
            "r7",
            "refund-7",
            0,
            json!({"kind":"refund","category_id":"food","amount":10,"date":"2026-10-19","original_operation_id":"expense","account_id":"wallet"}),
        )
        .unwrap();
    }
}
