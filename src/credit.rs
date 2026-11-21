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
fn schedule(
    principal: i64,
    first: &str,
    periods: u16,
    method: &str,
    rate_bps: u32,
    rate_type: &str,
) -> Result<Vec<Due>> {
    require(
        (1..=SAFE).contains(&principal)
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
        let interest = (remaining as f64 * rate).round() as i64;
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
            schedule(
                *principal, first_due, *periods, method, *rate_bps, rate_type,
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
                "INSERT INTO credit_loans VALUES(?,?,?,?,?,?,?,?,?,?,?)",
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
    let mut statement=conn.prepare("SELECT id,name,principal,account_id,draw_date,first_due,periods,method,rate_bps,rate_type FROM credit_loans WHERE user_id=? ORDER BY draw_date,id").map_err(database)?;
    let mut loans = statement.query([user]).map_err(database)?;
    let mut repayment_query=conn.prepare("SELECT id,date,principal,interest,fee FROM credit_repayments WHERE user_id=? AND loan_id=? ORDER BY date,id").map_err(database)?;
    while let Some(row) = loans.next().map_err(database)? {
        let id: String = row.get(0).map_err(database)?;
        let principal: i64 = row.get(2).map_err(database)?;
        let method: String = row.get(7).map_err(database)?;
        let rate_bps = row.get(8).map_err(database)?;
        let rate_type: String = row.get(9).map_err(database)?;
        let installments = schedule(
            principal,
            &row.get::<_, String>(5).map_err(database)?,
            row.get(6).map_err(database)?,
            &method,
            rate_bps,
            &rate_type,
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
    let mut query=conn.prepare("SELECT p.operation_id,p.first_due,p.periods,o.data FROM credit_purchases p JOIN finance_operations o ON p.user_id=o.user_id AND p.operation_id=o.id WHERE p.user_id=? ORDER BY p.first_due,p.operation_id").map_err(database)?;
    let mut rows = query.query([user]).map_err(database)?;
    while let Some(row) = rows.next().map_err(database)? {
        let op: finance::Operation =
            serde_json::from_str(&row.get::<_, String>(3).map_err(database)?)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if !op.deleted {
            result.purchases.push(Purchase {
                operation_id: op.id,
                installments: schedule(
                    op.amount,
                    &row.get::<_, String>(1).map_err(database)?,
                    row.get(2).map_err(database)?,
                    "german",
                    0,
                    "nominal",
                )?,
            });
        }
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
    let user = finance::bound_user(&state.db, &headers)?;
    Ok(Json(portfolio(&state.db, &user.id)?))
}
async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(command): Json<Command>,
) -> Result<StatusCode> {
    let user = finance::bound_user(&state.db, &headers)?;
    mutate(&state.db, &user.id, &command)?;
    Ok(StatusCode::NO_CONTENT)
}
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/credit", get(list).post(save))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_remainder_and_month_ends() {
        let dues = schedule(100, "2024-01-31", 3, "german", 0, "nominal").unwrap();
        assert_eq!(
            dues.iter()
                .map(|p| (p.date.as_str(), p.principal))
                .collect::<Vec<_>>(),
            vec![("2024-01-31", 34), ("2024-02-29", 33), ("2024-03-31", 33)]
        );
        assert!(schedule(100, "2023-02-29", 2, "french", 1000, "nominal").is_err());
    }
    #[test]
    fn french_and_german_nominal_reference() {
        let french = schedule(1200000, "2025-01-01", 3, "french", 1200, "nominal").unwrap();
        assert_eq!(
            french
                .iter()
                .map(|p| (p.principal, p.interest))
                .collect::<Vec<_>>(),
            vec![(396027, 12000), (399987, 8040), (403986, 4040)]
        );
        let german = schedule(1200000, "2025-01-01", 3, "german", 1200, "nominal").unwrap();
        assert_eq!(
            german
                .iter()
                .map(|p| (p.principal, p.interest))
                .collect::<Vec<_>>(),
            vec![(400000, 12000), (400000, 8000), (400000, 4000)]
        );
    }
    #[test]
    fn split_card_payment_cannot_exceed_total_and_edits_require_reallocation() {
        let db = Database::open_memory().unwrap();
        db.0.lock()
            .unwrap()
            .execute("INSERT INTO users VALUES('u','u','-',0)", [])
            .unwrap();
        for (id, kind) in [("cash", "cash"), ("card", "card")] {
            finance::save_account(
                &db,
                "u",
                &finance::Account {
                    id: id.into(),
                    kind: kind.into(),
                    name: id.into(),
                    initial_balance: 0,
                },
            )
            .unwrap();
        }
        let mut change: finance::Change=serde_json::from_value(serde_json::json!({
            "request_id":"payment-create","id":"p","expected_version":0,"deleted":false,"kind":"payment",
            "account_id":"cash","card_account_id":"card","amount":100,"date":"2024-02-20"
        })).unwrap();
        finance::apply(&db, "u", &change).unwrap();
        let allocate = |month: &str, amount: i64, expected_version: i64| Command {
            request_id: uuid::Uuid::new_v4().to_string(),
            entry: Entry::Allocate {
                operation_id: "p".into(),
                month: month.into(),
                amount,
                expected_version,
            },
        };
        mutate(&db, "u", &allocate("2024-01", 70, 1)).unwrap();
        assert_eq!(
            mutate(&db, "u", &allocate("2024-02", 40, 1)),
            Err(StatusCode::BAD_REQUEST)
        );
        let feb = allocate("2024-02", 30, 1);
        mutate(&db, "u", &feb).unwrap();
        mutate(&db, "u", &feb).unwrap();
        assert_eq!(
            portfolio(&db, "u")
                .unwrap()
                .allocations
                .iter()
                .map(|a| a.amount)
                .sum::<i64>(),
            100
        );
        change.request_id = "payment-edit".into();
        change.expected_version = 1;
        change.amount = Some(60);
        finance::apply(&db, "u", &change).unwrap();
        assert!(portfolio(&db, "u").unwrap().allocations.is_empty());
        assert_eq!(
            mutate(&db, "u", &allocate("2024-02", 30, 1)),
            Err(StatusCode::CONFLICT)
        );
        mutate(&db, "u", &allocate("2024-02", 60, 2)).unwrap();
        assert_eq!(portfolio(&db, "u").unwrap().allocations[0].amount, 60);
    }
    #[test]
    fn purchase_draw_partial_payment_retry_and_rollback() {
        let db = Database::open_memory().unwrap();
        db.0.lock()
            .unwrap()
            .execute("INSERT INTO users VALUES('a','a','-',0)", [])
            .unwrap();
        for (id, kind) in [("bank", "bank"), ("visa", "card")] {
            finance::save_account(
                &db,
                "a",
                &finance::Account {
                    id: id.into(),
                    kind: kind.into(),
                    initial_balance: 0,
                    name: id.into(),
                },
            )
            .unwrap();
        }
        finance::save_category(
            &db,
            "a",
            &finance::Category {
                id: "fee".into(),
                kind: "expense".into(),
                name: "Comisión".into(),
            },
        )
        .unwrap();
        let purchase = Command {
            request_id: uuid::Uuid::new_v4().to_string(),
            entry: Entry::Purchase {
                name: "Compra".into(),
                account_id: "visa".into(),
                category_id: "fee".into(),
                amount: 301,
                date: "2025-01-02".into(),
                first_due: "2025-01-31".into(),
                periods: 3,
            },
        };
        mutate(&db, "a", &purchase).unwrap();
        let loan = Command {
            request_id: uuid::Uuid::new_v4().to_string(),
            entry: Entry::Loan {
                name: "Préstamo".into(),
                account_id: "bank".into(),
                principal: 1000,
                date: "2025-01-02".into(),
                first_due: "2025-02-02".into(),
                periods: 2,
                method: "french".into(),
                rate_bps: 0,
                rate_type: "nominal".into(),
            },
        };
        mutate(&db, "a", &loan).unwrap();
        let mut payment = Command {
            request_id: uuid::Uuid::new_v4().to_string(),
            entry: Entry::Repayment {
                loan_id: loan.request_id.clone(),
                account_id: "bank".into(),
                date: "2025-02-01".into(),
                principal: 200,
                interest: 20,
                fee: 1,
                category_id: Some("missing".into()),
            },
        };
        assert!(mutate(&db, "a", &payment).is_err());
        assert_eq!(finance::snapshot(&db, "a").unwrap().operations.len(), 2);
        if let Entry::Repayment { category_id, .. } = &mut payment.entry {
            *category_id = Some("fee".into());
        }
        mutate(&db, "a", &payment).unwrap();
        mutate(&db, "a", &payment).unwrap();
        assert_eq!(finance::snapshot(&db, "a").unwrap().operations.len(), 5);
        let view = portfolio(&db, "a").unwrap();
        assert_eq!(
            view.purchases[0]
                .installments
                .iter()
                .map(|p| p.principal)
                .sum::<i64>(),
            301
        );
        assert_eq!(view.loans[0].outstanding, 800);
        assert_eq!(view.loans[0].payments.len(), 1);
        assert!(portfolio(&db, "b").unwrap().loans.is_empty());
        assert!(mutate(&db, "b", &payment).is_err());
    }

    #[test]
    fn two_identical_legitimate_repayments_are_both_persisted() {
        let db = Database::open_memory().unwrap();
        db.0.lock()
            .unwrap()
            .execute("INSERT INTO users VALUES('a','a','-',0)", [])
            .unwrap();
        finance::save_account(
            &db,
            "a",
            &finance::Account {
                id: "bank".into(),
                kind: "bank".into(),
                name: "bank".into(),
                initial_balance: 0,
            },
        )
        .unwrap();
        finance::save_category(
            &db,
            "a",
            &finance::Category {
                id: "fee".into(),
                kind: "expense".into(),
                name: "Comisión".into(),
            },
        )
        .unwrap();
        let loan = Command {
            request_id: uuid::Uuid::new_v4().to_string(),
            entry: Entry::Loan {
                name: "Préstamo".into(),
                account_id: "bank".into(),
                principal: 1000,
                date: "2025-01-02".into(),
                first_due: "2025-02-02".into(),
                periods: 2,
                method: "german".into(),
                rate_bps: 0,
                rate_type: "nominal".into(),
            },
        };
        mutate(&db, "a", &loan).unwrap();
        let repayment = |request_id: String| Command {
            request_id,
            entry: Entry::Repayment {
                loan_id: loan.request_id.clone(),
                account_id: "bank".into(),
                date: "2025-02-01".into(),
                principal: 200,
                interest: 10,
                fee: 1,
                category_id: Some("fee".into()),
            },
        };
        // Two different logical operations with identical data: both must be recorded.
        mutate(&db, "a", &repayment(uuid::Uuid::new_v4().to_string())).unwrap();
        mutate(&db, "a", &repayment(uuid::Uuid::new_v4().to_string())).unwrap();
        let view = portfolio(&db, "a").unwrap();
        assert_eq!(view.loans[0].payments.len(), 2);
        assert_eq!(view.loans[0].outstanding, 600);
        let conn = db.0.lock().unwrap();
        let ops: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM finance_operations WHERE user_id='a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ops, 7);
        let check: String = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(check, "ok");
        let fk: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(fk, 0);
    }

    #[test]
    fn cancelling_rectifies_purchases_and_repayments_atomically() {
        let db = Database::open_memory().unwrap();
        db.0.lock()
            .unwrap()
            .execute("INSERT INTO users VALUES('a','a','-',0)", [])
            .unwrap();
        for (id, kind) in [("bank", "bank"), ("visa", "card")] {
            finance::save_account(
                &db,
                "a",
                &finance::Account {
                    id: id.into(),
                    kind: kind.into(),
                    name: id.into(),
                    initial_balance: 0,
                },
            )
            .unwrap();
        }
        finance::save_category(
            &db,
            "a",
            &finance::Category {
                id: "fee".into(),
                kind: "expense".into(),
                name: "Comisión".into(),
            },
        )
        .unwrap();
        let purchase = Command {
            request_id: uuid::Uuid::new_v4().to_string(),
            entry: Entry::Purchase {
                name: "Compra".into(),
                account_id: "visa".into(),
                category_id: "fee".into(),
                amount: 300,
                date: "2025-01-02".into(),
                first_due: "2025-01-31".into(),
                periods: 3,
            },
        };
        mutate(&db, "a", &purchase).unwrap();
        let loan = Command {
            request_id: uuid::Uuid::new_v4().to_string(),
            entry: Entry::Loan {
                name: "Préstamo".into(),
                account_id: "bank".into(),
                principal: 1000,
                date: "2025-01-02".into(),
                first_due: "2025-02-02".into(),
                periods: 2,
                method: "german".into(),
                rate_bps: 0,
                rate_type: "nominal".into(),
            },
        };
        mutate(&db, "a", &loan).unwrap();
        let payment = Command {
            request_id: uuid::Uuid::new_v4().to_string(),
            entry: Entry::Repayment {
                loan_id: loan.request_id.clone(),
                account_id: "bank".into(),
                date: "2025-02-01".into(),
                principal: 200,
                interest: 20,
                fee: 1,
                category_id: Some("fee".into()),
            },
        };
        mutate(&db, "a", &payment).unwrap();
        assert_eq!(portfolio(&db, "a").unwrap().loans[0].outstanding, 800);

        // Unknown targets fail without touching anything.
        let unknown = Command {
            request_id: uuid::Uuid::new_v4().to_string(),
            entry: Entry::CancelPurchase {
                operation_id: "nope".into(),
            },
        };
        assert_eq!(
            mutate(&db, "a", &unknown).unwrap_err(),
            StatusCode::NOT_FOUND
        );

        // Cancel the purchase; retries are idempotent, the ledger keeps a tombstone.
        let purchase_op = format!("loan:{}:purchase", purchase.request_id);
        let cancel_purchase = Command {
            request_id: uuid::Uuid::new_v4().to_string(),
            entry: Entry::CancelPurchase {
                operation_id: purchase_op.clone(),
            },
        };
        mutate(&db, "a", &cancel_purchase).unwrap();
        mutate(&db, "a", &cancel_purchase).unwrap();
        assert!(portfolio(&db, "a").unwrap().purchases.is_empty());
        let snapshot = finance::snapshot(&db, "a").unwrap();
        assert!(
            snapshot
                .operations
                .iter()
                .any(|o| o.id == purchase_op && o.deleted)
        );

        // Cancel the repayment; outstanding capital and payments are restored.
        let cancel_payment = Command {
            request_id: uuid::Uuid::new_v4().to_string(),
            entry: Entry::CancelRepayment {
                repayment_id: payment.request_id.clone(),
            },
        };
        mutate(&db, "a", &cancel_payment).unwrap();
        mutate(&db, "a", &cancel_payment).unwrap();
        let view = portfolio(&db, "a").unwrap();
        assert_eq!(view.loans[0].outstanding, 1000);
        assert!(view.loans[0].payments.is_empty());
        let interest_id = format!("loan:{}:interest", payment.request_id);
        assert!(
            finance::snapshot(&db, "a")
                .unwrap()
                .operations
                .iter()
                .any(|o| o.id == interest_id && o.deleted)
        );

        // Another user cannot cancel what they never owned.
        let foreign = Command {
            request_id: uuid::Uuid::new_v4().to_string(),
            entry: Entry::CancelRepayment {
                repayment_id: payment.request_id.clone(),
            },
        };
        assert_eq!(
            mutate(&db, "b", &foreign).unwrap_err(),
            StatusCode::NOT_FOUND
        );

        // A refund on the purchase expense blocks its cancellation.
        let purchase2 = Command {
            request_id: uuid::Uuid::new_v4().to_string(),
            entry: Entry::Purchase {
                name: "Otra compra".into(),
                account_id: "visa".into(),
                category_id: "fee".into(),
                amount: 100,
                date: "2025-03-02".into(),
                first_due: "2025-03-31".into(),
                periods: 2,
            },
        };
        mutate(&db, "a", &purchase2).unwrap();
        let purchase2_op = format!("loan:{}:purchase", purchase2.request_id);
        let refund = finance::Change {
            id: "refund-1".into(),
            request_id: "refund-1".into(),
            expected_version: 0,
            deleted: false,
            kind: Some("refund".into()),
            account_id: Some("visa".into()),
            category_id: Some("fee".into()),
            card_account_id: None,
            destination_account_id: None,
            original_operation_id: Some(purchase2_op.clone()),
            amount: Some(50),
            date: Some("2025-03-05".into()),
            description: None,
            notes: None,
        };
        finance::apply(&db, "a", &refund).unwrap();
        let blocked = Command {
            request_id: uuid::Uuid::new_v4().to_string(),
            entry: Entry::CancelPurchase {
                operation_id: purchase2_op.clone(),
            },
        };
        assert_eq!(
            mutate(&db, "a", &blocked).unwrap_err(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(portfolio(&db, "a").unwrap().purchases.len(), 1);
    }
}
