//! Credit contracts and their cash movements share the same SQLite transaction.
use crate::{AppState, db::Database, finance};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::get,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

const SAFE: i64 = 9_007_199_254_740_991;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Entry {
    Purchase {
        name: String,
        account_id: String,
        category_id: String,
        amount: i64,
        date: String,
        first_due: String,
        periods: u16,
    },
    Loan {
        name: String,
        account_id: String,
        principal: i64,
        date: String,
        first_due: String,
        periods: u16,
        method: String,
        rate_bps: u32,
        rate_type: String,
    },
    Repayment {
        loan_id: String,
        account_id: String,
        date: String,
        principal: i64,
        interest: i64,
        fee: i64,
        category_id: Option<String>,
    },
    Allocate {
        operation_id: String,
        expected_version: i64,
        month: String,
        amount: i64,
    },
    CancelPurchase {
        operation_id: String,
    },
    CancelRepayment {
        repayment_id: String,
    },
    CancelLoan {
        loan_id: String,
    },
}
#[derive(Debug, Serialize, Deserialize)]
struct Command {
    request_id: String,
    entry: Entry,
}

#[derive(Debug, Serialize, PartialEq)]
struct Due {
    date: String,
    principal: i64,
    interest: i64,
}

#[derive(Debug, Serialize)]
struct Repayment {
    id: String,
    date: String,
    principal: i64,
    interest: i64,
    fee: i64,
}
#[derive(Debug, Serialize)]
struct Loan {
    id: String,
    name: String,
    principal: i64,
    outstanding: i64,
    account_id: String,
    draw_date: String,
    method: String,
    rate_bps: u32,
    rate_type: String,
    installments: Vec<Due>,
    payments: Vec<Repayment>,
}
#[derive(Debug, Serialize)]
struct Purchase {
    operation_id: String,
    installments: Vec<Due>,
}
#[derive(Debug, Serialize)]
struct Allocation {
    operation_id: String,
    month: String,
    amount: i64,
    payment_version: i64,
}
#[derive(Debug, Serialize)]
struct Portfolio {
    loans: Vec<Loan>,
    purchases: Vec<Purchase>,
    allocations: Vec<Allocation>,
}

type Result<T> = std::result::Result<T, StatusCode>;
fn database(_: rusqlite::Error) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}
fn require(valid: bool) -> Result<()> {
    if valid {
        Ok(())
    } else {
        Err(StatusCode::BAD_REQUEST)
    }
}
fn last_day(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        _ => 0,
    }
}
fn date_parts(date: &str) -> Result<(u32, u32, u32)> {
    require(finance::valid_date(date))?;
    let (y, m, d) = (
        date[..4].parse().unwrap(),
        date[5..7].parse().unwrap(),
        date[8..].parse().unwrap(),
    );
    Ok((y, m, d))
}
fn due_date(first: (u32, u32, u32), index: u16) -> Result<String> {
    let (y, m, d) = first;
    let absolute = y * 12 + m - 1 + u32::from(index);
    let (year, month) = (absolute / 12, absolute % 12 + 1);
    require(year <= 9999)?;
    Ok(format!(
        "{year:04}-{month:02}-{:02}",
        d.min(last_day(year, month))
    ))
}
const NOMINAL_DENOMINATOR: i128 = 120_000;

// V2: nominal monthly interest from an exact positive rational, half up.
fn nominal_interest(remaining: i64, rate_bps: u32) -> Result<i64> {
    let numerator = i128::from(remaining) * i128::from(rate_bps);
    let rounded = (numerator * 2 + NOMINAL_DENOMINATOR) / (NOMINAL_DENOMINATOR * 2);
    i64::try_from(rounded).map_err(|_| StatusCode::BAD_REQUEST)
}

// V1 is kept for loans created before migration 009; callers with a stored
// schedule_version must use schedule_v().
fn schedule(
    principal: i64,
    first: &str,
    periods: u16,
    method: &str,
    rate_bps: u32,
    rate_type: &str,
) -> Result<Vec<Due>> {
    schedule_v(principal, first, periods, method, rate_bps, rate_type, 1)
}

fn schedule_v(
    principal: i64,
    first: &str,
    periods: u16,
    method: &str,
    rate_bps: u32,
    rate_type: &str,
    version: u8,
) -> Result<Vec<Due>> {
    require(
        matches!(version, 1 | 2)
            && (1..=SAFE).contains(&principal)
            && (1..=600).contains(&periods)
            && i64::from(periods) <= principal
            && matches!(method, "french" | "german")
            && rate_bps <= 100_000
            && matches!(rate_type, "nominal" | "effective"),
    )?;
    let start = date_parts(first)?;
    let annual = f64::from(rate_bps) / 10000.0;
    let rate = if rate_type == "nominal" {
        annual / 12.0
    } else {
        (annual.ln_1p() / 12.0).exp_m1()
    };
    let n = i64::from(periods);
    let annuity = if rate > 0.0 {
        principal as f64 * rate / -(-f64::from(periods) * rate.ln_1p()).exp_m1()
    } else {
        0.0
    };
    let mut remaining = principal;
    let mut result = Vec::with_capacity(usize::from(periods));
    for i in 0..periods {
        let interest = if version == 2 && rate_type == "nominal" {
            nominal_interest(remaining, rate_bps)?
        } else {
            (remaining as f64 * rate).round() as i64
        };
        let capital = if i + 1 == periods {
            remaining
        } else if method == "german" || rate_bps == 0 {
            principal / n + i64::from(i64::from(i) < principal % n)
        } else {
            // Keep each installment payable in whole guaraníes, including the last one.
            (annuity.round() as i64 - interest).clamp(1, remaining - i64::from(periods - i - 1))
        };
        require(capital + interest <= SAFE)?;
        result.push(Due {
            date: due_date(start, i)?,
            principal: capital,
            interest,
        });
        remaining -= capital;
    }
    Ok(result)
}

fn operation(tx: &Connection, user: &str, id: &str) -> Result<Option<finance::Operation>> {
    tx.query_row(
        "SELECT data FROM finance_operations WHERE user_id=? AND id=?",
        params![user, id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(database)?
    .map(|s| serde_json::from_str(&s).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR))
    .transpose()
}
struct Posting<'a> {
    suffix: &'a str,
    kind: &'a str,
    name: &'a str,
    date: &'a str,
    account: &'a str,
    category: Option<&'a str>,
    amount: i64,
}
fn post(tx: &Connection, user: &str, request: &str, p: Posting<'_>) -> Result<String> {
    let id = format!("loan:{request}:{}", p.suffix);
    let change = finance::Change {
        id: id.clone(),
        request_id: id.clone(),
        expected_version: 0,
        deleted: false,
        kind: Some(p.kind.into()),
        account_id: Some(p.account.into()),
        category_id: p.category.map(String::from),
        card_account_id: None,
        destination_account_id: None,
        original_operation_id: None,
        amount: Some(p.amount),
        date: Some(p.date.into()),
        description: Some(p.name.into()),
        notes: None,
    };
    match finance::apply_in(tx, user, &change) {
        Ok(finance::Outcome::Applied(_)) => Ok(id),
        Ok(finance::Outcome::Conflict(_)) | Err(finance::Error::DuplicateRequest) => {
            Err(StatusCode::CONFLICT)
        }
        Err(finance::Error::Invalid) => Err(StatusCode::BAD_REQUEST),
        Err(finance::Error::Database) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}
fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 160 && !name.chars().any(char::is_control)
}
/// Tombstones a generated operation through the same versioned path as every other
/// edit, so refund invariants, balance checks and history all keep working.
fn cancel(tx: &Connection, user: &str, id: &str, version: i64) -> Result<()> {
    let change = finance::Change {
        id: id.to_owned(),
        request_id: format!("cancel:{id}:{version}"),
        expected_version: version,
        deleted: true,
        kind: None,
        account_id: None,
        category_id: None,
        card_account_id: None,
        destination_account_id: None,
        original_operation_id: None,
        amount: None,
        date: None,
        description: None,
        notes: None,
    };
    match finance::apply_in(tx, user, &change) {
        Ok(finance::Outcome::Applied(_)) => Ok(()),
        Ok(finance::Outcome::Conflict(_)) | Err(finance::Error::DuplicateRequest) => {
            Err(StatusCode::CONFLICT)
        }
        Err(finance::Error::Invalid) => Err(StatusCode::BAD_REQUEST),
        Err(finance::Error::Database) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}
fn mutate(db: &Database, user: &str, command: &Command) -> Result<()> {
    let id = &command.request_id;
    require(uuid::Uuid::parse_str(id).is_ok() && id.len() == 36)?;
    let encoded = serde_json::to_string(&command.entry).map_err(|_| StatusCode::BAD_REQUEST)?;
    let mut conn = db.0.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let tx = conn.transaction().map_err(database)?;
    let existing: Option<String> = tx
        .query_row(
            "SELECT payload FROM credit_requests WHERE user_id=? AND id=?",
            params![user, id],
            |r| r.get(0),
        )
        .optional()
        .map_err(database)?;
    if let Some(existing) = existing {
        return if existing == encoded {
            Ok(())
        } else {
            Err(StatusCode::CONFLICT)
        };
    }
    match &command.entry {
        Entry::Purchase {
            name,
            account_id,
            category_id,
            amount,
            date,
            first_due,
            periods,
        } => {
            require(valid_name(name) && first_due >= date)?;
            schedule(*amount, first_due, *periods, "german", 0, "nominal")?;
            let card:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM finance_accounts WHERE user_id=? AND id=? AND kind='card')",params![user,account_id],|r|r.get(0)).map_err(database)?;
            require(card)?;
            let op = post(
                &tx,
                user,
                id,
                Posting {
                    suffix: "purchase",
                    kind: "expense",
                    name,
                    date,
                    account: account_id,
                    category: Some(category_id),
                    amount: *amount,
                },
            )?;
            tx.execute(
                "INSERT INTO credit_purchases VALUES(?,?,?,?)",
                params![user, op, first_due, periods],
            )
            .map_err(database)?;
        }
        Entry::Loan {
            name,
            account_id,
            principal,
            date,
            first_due,
            periods,
            method,
            rate_bps,
            rate_type,
        } => {
            require(valid_name(name) && first_due >= date)?;
            schedule_v(
                *principal, first_due, *periods, method, *rate_bps, rate_type, 2,
            )?;
            post(
                &tx,
                user,
                id,
                Posting {
                    suffix: "draw",
                    kind: "loan_draw",
                    name,
                    date,
                    account: account_id,
                    category: None,
                    amount: *principal,
                },
            )?;
            tx.execute(
                "INSERT INTO credit_loans(user_id,id,name,account_id,principal,draw_date,first_due,periods,method,rate_bps,rate_type,schedule_version)
                 VALUES(?,?,?,?,?,?,?,?,?,?,?,2)",
                params![
                    user, id, name, account_id, principal, date, first_due, periods, method,
                    rate_bps, rate_type
                ],
            )
            .map_err(database)?;
        }
        Entry::Repayment {
            loan_id,
            account_id,
            date,
            principal,
            interest,
            fee,
            category_id,
        } => {
            require(
                [*principal, *interest, *fee]
                    .iter()
                    .all(|v| (0..=SAFE).contains(v))
                    && i128::from(*principal) + i128::from(*interest) + i128::from(*fee)
                        <= i128::from(SAFE)
                    && (*principal > 0 || *interest > 0 || *fee > 0),
            )?;
            date_parts(date)?;
            let account_valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM finance_accounts WHERE user_id=? AND id=? AND kind!='card')",params![user,account_id],|r|r.get(0)).map_err(database)?;
            require(account_valid)?;
            let (name,draw_date,remaining):(String,String,i64)=tx.query_row("SELECT name,draw_date,principal-COALESCE((SELECT SUM(p.principal) FROM credit_repayments p WHERE p.user_id=l.user_id AND p.loan_id=l.id),0) FROM credit_loans l WHERE user_id=? AND id=?",params![user,loan_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(database)?.ok_or(StatusCode::NOT_FOUND)?;
            require(
                *date >= draw_date
                    && *principal <= remaining
                    && (category_id.is_some() || (*interest == 0 && *fee == 0)),
            )?;
            for (suffix, kind, label, amount, category) in [
                ("principal", "loan_repayment", "Capital", *principal, None),
                (
                    "interest",
                    "expense",
                    "Interés",
                    *interest,
                    category_id.as_deref(),
                ),
                ("fee", "expense", "Comisión", *fee, category_id.as_deref()),
            ] {
                if amount > 0 {
                    post(
                        &tx,
                        user,
                        id,
                        Posting {
                            suffix,
                            kind,
                            name: &format!("{label} · {name}"),
                            date,
                            account: account_id,
                            category,
                            amount,
                        },
                    )?;
                }
            }
            tx.execute(
                "INSERT INTO credit_repayments VALUES(?,?,?,?,?,?,?)",
                params![user, id, loan_id, date, principal, interest, fee],
            )
            .map_err(database)?;
        }
        Entry::Allocate {
            operation_id,
            expected_version,
            month,
            amount,
        } => {
            date_parts(&format!("{month}-01"))?;
            let op = operation(&tx, user, operation_id)?.ok_or(StatusCode::NOT_FOUND)?;
            if op.version != *expected_version {
                return Err(StatusCode::CONFLICT);
            }
            require(!op.deleted && op.kind == "payment" && (0..=op.amount).contains(amount))?;
            tx.execute("DELETE FROM credit_allocations WHERE user_id=? AND operation_id=? AND (month=? OR payment_version!=?)",params![user,operation_id,month,op.version]).map_err(database)?;
            let assigned:i64=tx.query_row("SELECT COALESCE(SUM(amount),0) FROM credit_allocations WHERE user_id=? AND operation_id=?",params![user,operation_id],|r|r.get(0)).map_err(database)?;
            require(*amount <= op.amount - assigned)?;
            if *amount > 0 {
                tx.execute(
                    "INSERT INTO credit_allocations VALUES(?,?,?,?,?)",
                    params![user, operation_id, month, amount, op.version],
                )
                .map_err(database)?;
            }
        }
        Entry::CancelPurchase { operation_id } => {
            require(finance::id_ok(operation_id))?;
            let registered: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM credit_purchases WHERE user_id=?1 AND operation_id=?2)",
                    params![user, operation_id],
                    |r| r.get(0),
                )
                .map_err(database)?;
            if !registered {
                return Err(StatusCode::NOT_FOUND);
            }
            let op = operation(&tx, user, operation_id)?.ok_or(StatusCode::NOT_FOUND)?;
            require(!op.deleted)?;
            cancel(&tx, user, operation_id, op.version)?;
            tx.execute(
                "DELETE FROM credit_purchases WHERE user_id=?1 AND operation_id=?2",
                params![user, operation_id],
            )
            .map_err(database)?;
        }
        Entry::CancelRepayment { repayment_id } => {
            require(finance::id_ok(repayment_id))?;
            let registered: Option<String> = tx
                .query_row(
                    "SELECT id FROM credit_repayments WHERE user_id=?1 AND id=?2",
                    params![user, repayment_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(database)?;
            if registered.is_none() {
                return Err(StatusCode::NOT_FOUND);
            }
            for suffix in ["principal", "interest", "fee"] {
                let id = format!("loan:{repayment_id}:{suffix}");
                if let Some(op) = operation(&tx, user, &id)?
                    && !op.deleted
                {
                    cancel(&tx, user, &id, op.version)?;
                }
            }
            tx.execute(
                "DELETE FROM credit_repayments WHERE user_id=?1 AND id=?2",
                params![user, repayment_id],
            )
            .map_err(database)?;
        }
        Entry::CancelLoan { loan_id } => {
            require(finance::id_ok(loan_id))?;
            let registered: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM credit_loans WHERE user_id=?1 AND id=?2)",
                    params![user, loan_id],
                    |r| r.get(0),
                )
                .map_err(database)?;
            if !registered {
                return Err(StatusCode::NOT_FOUND);
            }
            let has_repayments: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM credit_repayments WHERE user_id=?1 AND loan_id=?2)",
                    params![user, loan_id],
                    |r| r.get(0),
                )
                .map_err(database)?;
            require(!has_repayments)?;
            let draw_id = format!("loan:{loan_id}:draw");
            if let Some(op) = operation(&tx, user, &draw_id)?
                && !op.deleted
            {
                cancel(&tx, user, &draw_id, op.version)?;
            }
            tx.execute(
                "DELETE FROM credit_loans WHERE user_id=?1 AND id=?2",
                params![user, loan_id],
            )
            .map_err(database)?;
        }
    }
    tx.execute(
        "INSERT INTO credit_requests VALUES(?,?,?)",
        params![user, id, encoded],
    )
    .map_err(database)?;
    tx.commit().map_err(database)
}

fn portfolio(db: &Database, user: &str) -> Result<Portfolio> {
    let conn = db.0.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut result = Portfolio {
        loans: vec![],
        purchases: vec![],
        allocations: vec![],
    };
    let mut statement=conn.prepare("SELECT id,name,principal,account_id,draw_date,first_due,periods,method,rate_bps,rate_type,schedule_version FROM credit_loans WHERE user_id=? ORDER BY draw_date,id").map_err(database)?;
    let mut loans = statement.query([user]).map_err(database)?;
    let mut repayment_query=conn.prepare("SELECT id,date,principal,interest,fee FROM credit_repayments WHERE user_id=? AND loan_id=? ORDER BY date,id").map_err(database)?;
    while let Some(row) = loans.next().map_err(database)? {
        let id: String = row.get(0).map_err(database)?;
        let principal: i64 = row.get(2).map_err(database)?;
        let method: String = row.get(7).map_err(database)?;
        let rate_bps = row.get(8).map_err(database)?;
        let rate_type: String = row.get(9).map_err(database)?;
        let version: u8 = row.get(10).map_err(database)?;
        let installments = schedule_v(
            principal,
            &row.get::<_, String>(5).map_err(database)?,
            row.get(6).map_err(database)?,
            &method,
            rate_bps,
            &rate_type,
            version,
        )?;
        let payments = repayment_query
            .query_map(params![user, id], |r| {
                Ok(Repayment {
                    id: r.get(0)?,
                    date: r.get(1)?,
                    principal: r.get(2)?,
                    interest: r.get(3)?,
                    fee: r.get(4)?,
                })
            })
            .map_err(database)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(database)?;
        let outstanding = principal - payments.iter().map(|p| p.principal).sum::<i64>();
        result.loans.push(Loan {
            id,
            name: row.get(1).map_err(database)?,
            account_id: row.get(3).map_err(database)?,
            draw_date: row.get(4).map_err(database)?,
            principal,
            outstanding,
            method,
            rate_bps,
            rate_type,
            installments,
            payments,
        });
    }
    let mut query=conn.prepare_cached("SELECT p.operation_id,p.first_due,p.periods,o.amount FROM credit_purchases p JOIN finance_operations o ON p.user_id=o.user_id AND p.operation_id=o.id WHERE p.user_id=? AND o.deleted=0 ORDER BY p.first_due,p.operation_id").map_err(database)?;
    let mut rows = query.query([user]).map_err(database)?;
    while let Some(row) = rows.next().map_err(database)? {
        result.purchases.push(Purchase {
            operation_id: row.get(0).map_err(database)?,
            installments: schedule(
                row.get(3).map_err(database)?,
                &row.get::<_, String>(1).map_err(database)?,
                row.get(2).map_err(database)?,
                "german",
                0,
                "nominal",
            )?,
        });
    }
    let mut query=conn.prepare("SELECT a.operation_id,a.month,a.amount,a.payment_version FROM credit_allocations a JOIN finance_operations o ON o.user_id=a.user_id AND o.id=a.operation_id AND o.version=a.payment_version WHERE a.user_id=? AND o.deleted=0 AND o.kind='payment' ORDER BY a.month,a.operation_id").map_err(database)?;
    result.allocations = query
        .query_map([user], |r| {
            Ok(Allocation {
                operation_id: r.get(0)?,
                month: r.get(1)?,
                amount: r.get(2)?,
                payment_version: r.get(3)?,
            })
        })
        .map_err(database)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(database)?;
    Ok(result)
}
async fn list(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Portfolio>> {
    tokio::task::spawn_blocking(move || {
        let user = finance::bound_user(&state.db, &headers)?;
        Ok(Json(portfolio(&state.db, &user.id)?))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
}
async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(command): Json<Command>,
) -> Result<StatusCode> {
    tokio::task::spawn_blocking(move || {
        let user = finance::bound_user(&state.db, &headers)?;
        mutate(&state.db, &user.id, &command)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
}
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/credit", get(list).post(save))
}
