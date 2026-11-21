//! Monthly planning, document storage and review of confirmed ledger entries.
use crate::{
    AppState, catalog,
    db::Database,
    finance::{self, Change, Operation, Outcome},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const MAX_AMOUNT: i64 = 9_007_199_254_740_991;
const MAX_FILE: usize = 5 * 1024 * 1024;
// Receipts live inside SQLite and travel in every backup; a per-user cap keeps
// both bounded on the NAS volume.
const MAX_TOTAL_RECEIPTS: i64 = 200 * 1024 * 1024;

#[derive(Debug)]
pub(crate) enum Error {
    Invalid,
    Missing,
    Stale,
    Database,
    Unauthorized,
    Forbidden,
}
impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self {
        Self::Database
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        match self {
            Self::Invalid => StatusCode::BAD_REQUEST,
            Self::Missing => StatusCode::NOT_FOUND,
            Self::Stale => StatusCode::CONFLICT,
            Self::Database => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
        }
        .into_response()
    }
}
fn user(db: &Database, headers: &HeaderMap) -> Result<String, Error> {
    finance::bound_user(db, headers)
        .map(|user| user.id)
        .map_err(|status| {
            if status == StatusCode::UNAUTHORIZED {
                Error::Unauthorized
            } else {
                Error::Forbidden
            }
        })
}
fn days(month: &str) -> Option<u8> {
    let b = month.as_bytes();
    if b.len() != 7 || b[4] != b'-' || !b[..4].iter().chain(&b[5..]).all(u8::is_ascii_digit) {
        return None;
    }
    let y = month[..4].parse::<u16>().ok()?;
    if y == 0 {
        return None;
    }
    match &month[5..] {
        "02" => Some(if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            29
        } else {
            28
        }),
        "04" | "06" | "09" | "11" => Some(30),
        "01" | "03" | "05" | "07" | "08" | "10" | "12" => Some(31),
        _ => None,
    }
}
fn valid_name(v: &str) -> bool {
    !v.trim().is_empty() && v.len() <= 180 && !v.chars().any(char::is_control)
}
fn valid_id(v: &str) -> bool {
    !v.is_empty() && v.len() <= 128 && !v.chars().any(char::is_control)
}
fn valid_amount(v: i64) -> bool {
    (1..=MAX_AMOUNT).contains(&v)
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Plan {
    Budget {
        month: String,
        category_id: String,
        amount: i64,
    },
    Goal {
        id: String,
        name: String,
        account_id: String,
        target: i64,
        deadline: String,
    },
    Recurring {
        id: String,
        name: String,
        account_id: String,
        category_id: String,
        amount: i64,
        first_month: String,
        last_month: Option<String>,
        day: u8,
    },
    ConfirmRecurring {
        id: String,
        month: String,
    },
    RemoveGoal {
        id: String,
    },
    RemoveRecurring {
        id: String,
    },
    RemoveBudget {
        month: String,
        category_id: String,
    },
}

/// A delete that matched nothing is a 404, not a silent success.
fn removed(conn: &Connection, sql: &str, params: impl rusqlite::Params) -> Result<(), Error> {
    if conn.execute(sql, params)? == 0 {
        return Err(Error::Missing);
    }
    Ok(())
}

fn save(conn: &Connection, u: &str, plan: Plan) -> Result<(), Error> {
    let permitted_account = |id: &str| -> Result<bool, Error> {
        Ok(!catalog::is_archived(conn,u,"account",id)? && conn.query_row("SELECT EXISTS(SELECT 1 FROM finance_accounts WHERE user_id=?1 AND id=?2 AND kind!='card')",params![u,id],|r|r.get(0))?)
    };
    let expense_category = |id: &str| -> Result<bool, Error> {
        Ok(!catalog::is_archived(conn,u,"category",id)? && conn.query_row("SELECT EXISTS(SELECT 1 FROM finance_categories WHERE user_id=?1 AND id=?2 AND kind='expense')",params![u,id],|r|r.get(0))?)
    };
    match plan {
        Plan::Budget {
            month,
            category_id,
            amount,
        } if days(&month).is_some() && valid_amount(amount) && expense_category(&category_id)? => {
            conn.execute("INSERT INTO planning_budgets VALUES(?1,?2,?3,?4) ON CONFLICT(user_id,month,category_id) DO UPDATE SET amount=excluded.amount",params![u,month,category_id,amount])?;
        }
        Plan::Goal {
            id,
            name,
            account_id,
            target,
            deadline,
        } if valid_id(&id)
            && valid_name(&name)
            && valid_amount(target)
            && days(&deadline).is_some()
            && permitted_account(&account_id)? =>
        {
            conn.execute("INSERT INTO planning_goals VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(user_id,id) DO UPDATE SET name=excluded.name,account_id=excluded.account_id,target=excluded.target,deadline=excluded.deadline",params![u,id,name.trim(),account_id,target,deadline])?;
        }
        Plan::Recurring {
            id,
            name,
            account_id,
            category_id,
            amount,
            first_month,
            last_month,
            day,
        } if valid_id(&id)
            && valid_name(&name)
            && valid_amount(amount)
            && days(&first_month).is_some()
            && last_month
                .as_ref()
                .is_none_or(|m| days(m).is_some() && m >= &first_month)
            && (1..=31).contains(&day)
            && permitted_account(&account_id)?
            && expense_category(&category_id)? =>
        {
            conn.execute("INSERT INTO planning_recurring VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(user_id,id) DO UPDATE SET name=excluded.name,account_id=excluded.account_id,category_id=excluded.category_id,amount=excluded.amount,first_month=excluded.first_month,last_month=excluded.last_month,day=excluded.day",params![u,id,name.trim(),account_id,category_id,amount,first_month,last_month,day])?;
        }
        Plan::RemoveGoal { id } => {
            removed(
                conn,
                "DELETE FROM planning_goals WHERE user_id=?1 AND id=?2",
                params![u, id],
            )?;
        }
        Plan::RemoveRecurring { id } => {
            removed(
                conn,
                "DELETE FROM planning_recurring WHERE user_id=?1 AND id=?2",
                params![u, id],
            )?;
        }
        Plan::RemoveBudget { month, category_id } => {
            removed(
                conn,
                "DELETE FROM planning_budgets WHERE user_id=?1 AND month=?2 AND category_id=?3",
                params![u, month, category_id],
            )?;
        }
        Plan::ConfirmRecurring { id, month } => confirm_recurring(conn, u, &id, &month)?,
        _ => return Err(Error::Invalid),
    }
    Ok(())
}
async fn save_plan(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(v): Json<Plan>,
) -> Result<StatusCode, Error> {
    let u = user(&s.db, &h)?;
    let mut conn = s.db.0.lock().map_err(|_| Error::Database)?;
    let tx = conn.transaction()?;
    save(&tx, &u, v)?;
    tx.commit()?;
    Ok(StatusCode::NO_CONTENT)
}

fn recurrence_id(id: &str, month: &str) -> String {
    format!(
        "rec-{}",
        hex::encode(Sha256::digest(format!("{id}/{month}").as_bytes()))
    )
}

fn confirm_recurring(conn: &Connection, user: &str, id: &str, month: &str) -> Result<(), Error> {
    let last_day = days(month).ok_or(Error::Invalid)?;
    let (account_id, category_id, name, amount, day): (String, String, String, i64, u8) = conn
        .query_row(
            "SELECT account_id,category_id,name,amount,day FROM planning_recurring
         WHERE user_id=?1 AND id=?2 AND first_month<=?3 AND (last_month IS NULL OR last_month>=?3)",
            params![user, id, month],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?
        .ok_or(Error::Missing)?;
    let operation_id = recurrence_id(id, month);
    let previous = conn
        .query_row(
            "SELECT version,deleted FROM finance_operations WHERE user_id=? AND id=?",
            params![user, operation_id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, bool>(1)?)),
        )
        .optional()?;
    if previous.is_some_and(|(_, deleted)| !deleted) {
        return Ok(());
    }
    let version = previous.map_or(0, |(version, _)| version);
    let change = Change {
        request_id: format!("{operation_id}:{version}"),
        id: operation_id,
        expected_version: version,
        deleted: false,
        kind: Some("expense".into()),
        account_id: Some(account_id),
        category_id: Some(category_id),
        amount: Some(amount),
        date: Some(format!("{month}-{:02}", day.min(last_day))),
        description: Some(name),
        card_account_id: None,
        destination_account_id: None,
        original_operation_id: None,
        notes: None,
    };
    match finance::apply_in(conn, user, &change) {
        Ok(Outcome::Applied(_)) => Ok(()),
        Ok(Outcome::Conflict(_)) | Err(finance::Error::DuplicateRequest) => Err(Error::Stale),
        Err(finance::Error::Invalid) => Err(Error::Invalid),
        Err(finance::Error::Database) => Err(Error::Database),
    }
}

pub(crate) fn report(conn: &Connection, u: &str, month: &str) -> Result<Value, Error> {
    let last_day = days(month).ok_or(Error::Invalid)?;
    let start = format!("{month}-01");
    let end = format!("{month}-{last_day:02}");
    let mut query=conn.prepare("SELECT data FROM finance_operations WHERE user_id=?1 AND date BETWEEN ?2 AND ?3 ORDER BY id")?;
    // Prefixed so every review stored before v2 stops matching once and is reviewed again.
    let mut digest = Sha256::new();
    digest.update(b"finapp-review-v2");
    let mut income = 0i128;
    let mut expense = 0i128;
    let mut loan_principal = 0i128;
    let mut loan_interest = 0i128;
    let mut loan_fees = 0i128;
    let mut categories = BTreeMap::<String, i128>::new();
    let mut accounts = BTreeMap::<String, i128>::new();
    let mut operations = Vec::new();
    for row in query.query_map(params![u, start, end], |r| r.get::<_, String>(0))? {
        let data = row?;
        digest.update((data.len() as u64).to_be_bytes());
        digest.update(data.as_bytes());
        let op: Operation = serde_json::from_str(&data).map_err(|_| Error::Database)?;
        if op.deleted {
            continue;
        }
        let value = i128::from(op.amount);
        match op.kind.as_str() {
            "income" => income += value,
            "loan_repayment" => loan_principal += value,
            "expense" | "refund" => {
                let net = if op.kind == "refund" { -value } else { value };
                expense += net;
                if op.id.starts_with("loan:") && op.id.ends_with(":interest")
                    || op.id.starts_with("xlsx-interes-")
                {
                    loan_interest += net;
                }
                if op.id.starts_with("loan:") && op.id.ends_with(":fee")
                    || op.id.starts_with("xlsx-comision-")
                {
                    loan_fees += net;
                }
                *accounts.entry(op.account_id.clone()).or_default() += net;
                if let Some(id) = &op.category_id {
                    *categories.entry(id.clone()).or_default() += net;
                }
            }
            _ => {}
        }
        operations.push(op);
    }
    let mut stmt=conn.prepare("SELECT category_id,amount FROM planning_budgets WHERE user_id=?1 AND month=?2 ORDER BY category_id")?;
    let budgets = stmt
        .query_map(params![u, month], |r| {
            Ok(json!({"category_id":r.get::<_,String>(0)?,"amount":r.get::<_,i64>(1)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut stmt=conn.prepare("SELECT id,name,account_id,target,deadline FROM planning_goals WHERE user_id=?1 ORDER BY name,id")?;
    let goals=stmt.query_map([u],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"account_id":r.get::<_,String>(2)?,"target":r.get::<_,i64>(3)?,"deadline":r.get::<_,String>(4)?})))?.collect::<Result<Vec<_>,_>>()?;
    let mut stmt=conn.prepare("SELECT id,name,account_id,category_id,amount,first_month,last_month,day FROM planning_recurring WHERE user_id=?1 ORDER BY day,name,id")?;
    let recurring=stmt.query_map([u],|r|{
        let id=r.get::<_,String>(0)?;
        let first=r.get::<_,String>(5)?;
        let last=r.get::<_,Option<String>>(6)?;
        let day=r.get::<_,u8>(7)?;
        let due=(first.as_str()<=month && last.as_ref().is_none_or(|m|m.as_str()>=month)).then(||format!("{month}-{:02}",day.min(last_day)));
        let op_id=recurrence_id(&id,month);
        let posted:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM finance_operations WHERE user_id=?1 AND id=?2 AND deleted=0)",params![u,op_id],|row|row.get(0))?;
        Ok(json!({"id":id,"name":r.get::<_,String>(1)?,"account_id":r.get::<_,String>(2)?,"category_id":r.get::<_,String>(3)?,"amount":r.get::<_,i64>(4)?,"first_month":first,"last_month":last,"day":day,"due":due,"posted":posted,"operation_id":op_id}))
    })?.collect::<Result<Vec<_>,_>>()?;
    let mut stmt=conn.prepare("SELECT p.id,p.operation_id,p.name,p.mime,length(p.content),p.digest FROM planning_receipts p JOIN finance_operations o ON o.user_id=p.user_id AND o.id=p.operation_id WHERE p.user_id=?1 AND substr(json_extract(o.data,'$.date'),1,7)=?2 AND json_extract(o.data,'$.deleted')=0 ORDER BY p.operation_id,p.id")?;
    let receipts=stmt.query_map(params![u,month],|r|Ok(json!({"id":r.get::<_,String>(0)?,"operation_id":r.get::<_,String>(1)?,"name":r.get::<_,String>(2)?,"mime":r.get::<_,String>(3)?,"size":r.get::<_,i64>(4)? as usize,"digest":r.get::<_,String>(5)?})))?.collect::<Result<Vec<_>,_>>()?;
    digest.update(serde_json::to_vec(&(&budgets, &receipts)).map_err(|_| Error::Database)?);
    let balances = finance::balances_in(conn, u, Some(&end))?;
    let mut stmt = conn.prepare("SELECT id,kind FROM finance_accounts WHERE user_id=?")?;
    let mut net_worth = 0i128;
    for row in stmt.query_map([u], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        let (id, kind) = row?;
        let balance = balances[&id];
        net_worth += if kind == "card" { -balance } else { balance };
    }
    let mut loan_debt = 0i128;
    let mut stmt=conn.prepare("SELECT principal-COALESCE((SELECT SUM(p.principal) FROM credit_repayments p
        WHERE l.user_id=p.user_id AND l.id=p.loan_id AND p.date<=?2),0) FROM credit_loans l WHERE l.user_id=?1 AND l.draw_date<=?2")?;
    for row in stmt.query_map(params![u, end], |r| r.get::<_, i64>(0))? {
        loan_debt += i128::from(row?);
    }
    net_worth -= loan_debt;
    let mut cash_flow = 0i128;
    let mut stmt=conn.prepare("SELECT p.delta FROM finance_postings p JOIN finance_accounts a ON p.user_id=a.user_id AND p.account_id=a.id
        WHERE p.user_id=? AND a.kind!='card' AND p.date BETWEEN ? AND ?")?;
    for row in stmt.query_map(params![u, start, end], |r| r.get::<_, i64>(0))? {
        cash_flow += i128::from(row?);
    }
    let mut stmt=conn.prepare("SELECT changed_at,previous,current FROM finance_operation_history
        WHERE user_id=?1 AND (substr(json_extract(previous,'$.date'),1,7)=?2 OR substr(json_extract(current,'$.date'),1,7)=?2)
        ORDER BY seq DESC LIMIT 100")?;
    let changes=stmt.query_map(params![u,month],|r| {
        Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?))
    })?.map(|row|->Result<Value,Error>{
        let (at,old,new)=row?;
        let parse=|s:&str|serde_json::from_str::<Operation>(s).map_err(|_|Error::Database);
        Ok(json!({"at":at,"previous":old.as_deref().map(parse).transpose()?,"current":parse(&new)?}))
    }).collect::<Result<Vec<_>,_>>()?;
    // Balances and derived totals are part of the digest: an edit in an earlier month
    // changes later months' net worth, and must return those months to pending review.
    for (id, balance) in &balances {
        digest.update((id.len() as u64).to_be_bytes());
        digest.update(id.as_bytes());
        let value = balance.to_string();
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value.as_bytes());
    }
    for total in [net_worth, loan_debt, cash_flow] {
        let value = total.to_string();
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value.as_bytes());
    }
    let digest = hex::encode(digest.finalize());
    let reviewed = conn
        .query_row(
            "SELECT digest FROM planning_reviews WHERE user_id=?1 AND month=?2",
            params![u, month],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .is_some_and(|s| s == digest);
    let as_decimal = |map: BTreeMap<String, i128>| {
        map.into_iter()
            .map(|(k, v)| (k, v.to_string()))
            .collect::<BTreeMap<_, _>>()
    };
    Ok(json!({
        "month":month, "income":income.to_string(), "expense":expense.to_string(), "net":(income-expense).to_string(),
        "cash_flow":cash_flow.to_string(),"net_worth":net_worth.to_string(),"loan_debt":loan_debt.to_string(),
        "loan_principal":loan_principal.to_string(),"loan_interest":loan_interest.to_string(),"loan_fees":loan_fees.to_string(),
        "categories":as_decimal(categories),"accounts":as_decimal(accounts),"balances":as_decimal(balances),
        "budgets":budgets,"goals":goals,"recurring":recurring,"operations":operations,"receipts":receipts,
        "changes":changes,"digest":digest,"reviewed":reviewed
    }))
}
async fn get_report(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(month): Path<String>,
) -> Result<Json<Value>, Error> {
    let u = user(&s.db, &h)?;
    let conn = s.db.0.lock().map_err(|_| Error::Database)?;
    Ok(Json(report(&conn, &u, &month)?))
}
#[derive(Deserialize)]
struct Review {
    digest: String,
}
async fn review(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(month): Path<String>,
    Json(v): Json<Review>,
) -> Result<StatusCode, Error> {
    let u = user(&s.db, &h)?;
    let mut conn = s.db.0.lock().map_err(|_| Error::Database)?;
    let tx = conn.transaction()?;
    if report(&tx, &u, &month)?["digest"].as_str() != Some(&v.digest) {
        return Err(Error::Stale);
    }
    tx.execute("INSERT INTO planning_reviews VALUES(?1,?2,?3) ON CONFLICT(user_id,month) DO UPDATE SET digest=excluded.digest",params![u,month,v.digest])?;
    tx.commit()?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
struct Upload {
    operation_id: String,
    name: String,
    mime: String,
    content: String,
}
fn mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"%PDF-") {
        Some("application/pdf")
    } else {
        None
    }
}
fn store_receipt(conn: &Connection, u: &str, v: Upload) -> Result<String, Error> {
    if !valid_name(&v.name) || v.name.contains(['/', '\\']) || v.content.len() > 2 * MAX_FILE {
        return Err(Error::Invalid);
    }
    let bytes = hex::decode(&v.content).map_err(|_| Error::Invalid)?;
    if mime(&bytes) != Some(v.mime.as_str()) {
        return Err(Error::Invalid);
    }
    let stored: i64 = conn.query_row(
        "SELECT COALESCE(SUM(length(content)),0) FROM planning_receipts WHERE user_id=?1",
        params![u],
        |r| r.get(0),
    )?;
    if stored.saturating_add(i64::try_from(bytes.len()).unwrap_or(i64::MAX)) > MAX_TOTAL_RECEIPTS {
        return Err(Error::Invalid);
    }
    let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM finance_operations WHERE user_id=?1 AND id=?2 AND json_extract(data,'$.deleted')=0)",params![u,v.operation_id],|r|r.get(0))?;
    if !exists {
        return Err(Error::Missing);
    }
    let digest = hex::encode(Sha256::digest(&bytes));
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute("INSERT INTO planning_receipts VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(user_id,operation_id,digest) DO NOTHING",params![u,id,v.operation_id,v.name,v.mime,digest,bytes])?;
    Ok(conn.query_row(
        "SELECT id FROM planning_receipts WHERE user_id=?1 AND operation_id=?2 AND digest=?3",
        params![u, v.operation_id, digest],
        |r| r.get(0),
    )?)
}
async fn upload(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(v): Json<Upload>,
) -> Result<Json<Value>, Error> {
    let u = user(&s.db, &h)?;
    let conn = s.db.0.lock().map_err(|_| Error::Database)?;
    Ok(Json(json!({"id":store_receipt(&conn,&u,v)?})))
}
async fn download(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let u = user(&s.db, &h)?;
    let conn = s.db.0.lock().map_err(|_| Error::Database)?;
    let (mime,bytes):(String,Vec<u8>)=conn.query_row("SELECT p.mime,p.content FROM planning_receipts p JOIN finance_operations o ON o.user_id=p.user_id AND o.id=p.operation_id WHERE p.user_id=?1 AND p.id=?2 AND json_extract(o.data,'$.deleted')=0",params![u,id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?.ok_or(Error::Missing)?;
    Ok((
        [
            (header::CONTENT_TYPE, mime.as_str()),
            (header::CONTENT_DISPOSITION, "attachment"),
        ],
        bytes,
    )
        .into_response())
}
async fn remove_receipt(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, Error> {
    let u = user(&s.db, &h)?;
    let conn = s.db.0.lock().map_err(|_| Error::Database)?;
    removed(
        &conn,
        "DELETE FROM planning_receipts WHERE user_id=?1 AND id=?2",
        params![u, id],
    )?;
    Ok(StatusCode::NO_CONTENT)
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/planning/month/{month}", get(get_report).post(review))
        .route("/api/planning", post(save_plan))
        .route(
            "/api/receipts",
            post(upload).layer(DefaultBodyLimit::max(2 * MAX_FILE + 8192)),
        )
        .route("/api/receipts/{id}", get(download).delete(remove_receipt))
}
