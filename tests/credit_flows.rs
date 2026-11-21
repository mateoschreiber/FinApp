//! F9.2B: loans, disbursements, repayments and cancellations through the real
//! Axum router. Expected numbers are literals computed independently; persisted
//! state is checked with raw SQL, never with production portfolio()/report().
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
};
use finapp::{app, db::Database};
use http_body_util::BodyExt;
use rusqlite::params;
use serde_json::{Value, json};
use tower::ServiceExt;

const ORIGIN: &str = "https://finanzas.example";
const PASSWORD: &str = "contraseña temporal sintética";
const MONTH: &str = "2026-11";

struct Session {
    id: String,
    cookie: String,
}

struct Fixture {
    db: Database,
    app: Router,
    admin: Session,
}

impl Fixture {
    async fn new() -> Self {
        let db = Database::open_memory().unwrap();
        db.bootstrap_admin("admin", PASSWORD).unwrap();
        let app = app(db.clone(), ORIGIN.to_owned());
        let admin = Self::login(&app, "admin", PASSWORD).await;
        Self { db, app, admin }
    }

    async fn login(app: &Router, username: &str, password: &str) -> Session {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/login")
                    .header("origin", ORIGIN)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"username":username,"password":password}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let cookie = res
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let id = serde_json::from_slice::<Value>(&body).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        Session { id, cookie }
    }

    async fn new_user(&self, username: &str) -> Session {
        let res = self
            .request(
                &self.admin,
                "POST",
                "/api/users",
                json!({"username":username,"password":PASSWORD}),
            )
            .await;
        assert_eq!(res.status(), StatusCode::CREATED);
        Self::login(&self.app, username, PASSWORD).await
    }

    async fn request(&self, session: &Session, method: &str, path: &str, body: Value) -> Response {
        let mut req = Request::builder().method(method).uri(path);
        if method != "GET" {
            req = req
                .header("origin", ORIGIN)
                .header("content-type", "application/json");
        }
        req = req
            .header("cookie", &session.cookie)
            .header("x-finapp-user", &session.id);
        self.app
            .clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    async fn send(&self, session: &Session, entry: Value, request_id: &str) -> StatusCode {
        let res = self
            .request(
                session,
                "POST",
                "/api/credit",
                json!({"request_id":request_id,"entry":entry}),
            )
            .await;
        res.status()
    }

    async fn send_new(&self, session: &Session, entry: Value) -> (StatusCode, String) {
        let id = uuid::Uuid::new_v4().to_string();
        let status = self.send(session, entry, &id).await;
        (status, id)
    }

    async fn portfolio(&self, session: &Session) -> Value {
        let res = self.request(session, "GET", "/api/credit", json!({})).await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&body).unwrap()
    }

    async fn report(&self, session: &Session) -> Value {
        self.report_for(session, MONTH).await
    }

    async fn report_for(&self, session: &Session, month: &str) -> Value {
        let res = self
            .request(
                session,
                "GET",
                &format!("/api/planning/month/{month}"),
                json!({}),
            )
            .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&body).unwrap()
    }

    async fn operation(&self, session: &Session, change: Value) -> StatusCode {
        let res = self
            .request(session, "POST", "/api/operations", change)
            .await;
        res.status()
    }

    fn details(
        &self,
        user: &str,
        account: &str,
        closing_day: Option<u8>,
        due_day: Option<u8>,
        cutoff: Option<&str>,
    ) {
        self.db
            .0
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO catalog_details(user_id,entity,id,archived,institution,notes,cutoff_date,closing_day,due_day)
                 VALUES(?1,'account',?2,0,'','',?3,?4,?5)
                 ON CONFLICT(user_id,entity,id) DO UPDATE SET cutoff_date=excluded.cutoff_date,
                 closing_day=excluded.closing_day, due_day=excluded.due_day",
                params![user, account, cutoff, closing_day, due_day],
            )
            .unwrap();
    }

    fn card_balance(&self, user: &str, card: &str) -> i64 {
        self.balance(user, card)
    }

    fn account(&self, user: &str, id: &str, kind: &str, initial: i64) {
        finapp::finance::save_account(
            &self.db,
            user,
            &finapp::finance::Account {
                id: id.into(),
                name: id.into(),
                kind: kind.into(),
                initial_balance: initial,
            },
        )
        .unwrap();
    }

    fn category(&self, user: &str, id: &str) {
        finapp::finance::save_category(
            &self.db,
            user,
            &finapp::finance::Category {
                id: id.into(),
                name: id.into(),
                kind: "expense".into(),
            },
        )
        .unwrap();
    }

    fn balance(&self, user: &str, account: &str) -> i64 {
        let conn = self.db.0.lock().unwrap();
        let initial: i64 = conn
            .query_row(
                "SELECT initial_balance FROM finance_accounts WHERE user_id=?1 AND id=?2",
                params![user, account],
                |r| r.get(0),
            )
            .unwrap();
        let delta: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(delta),0) FROM finance_balance_postings WHERE user_id=?1 AND account_id=?2",
                params![user, account],
                |r| r.get(0),
            )
            .unwrap();
        initial + delta
    }

    fn scalar(&self, sql: &str, user: &str) -> i64 {
        self.db
            .0
            .lock()
            .unwrap()
            .query_row(sql, params![user], |r| r.get(0))
            .unwrap()
    }

    fn postings(&self, user: &str) -> Vec<(String, String, i64)> {
        let conn = self.db.0.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id,account_id,delta FROM finance_postings WHERE user_id=?1 ORDER BY id,account_id",
            )
            .unwrap();
        stmt.query_map([user], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn integrity(&self) {
        let conn = self.db.0.lock().unwrap();
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
}

fn loan(principal: i64, account: &str, date: &str, rate_bps: u32) -> Value {
    json!({
        "kind":"loan", "name":"Préstamo", "account_id":account,
        "principal":principal, "date":date, "first_due":"2026-12-01",
        "periods":2, "method":"german", "rate_bps":rate_bps, "rate_type":"nominal"
    })
}

fn repayment(
    loan_id: &str,
    principal: i64,
    interest: i64,
    fee: i64,
    category: Option<&str>,
) -> Value {
    json!({
        "kind":"repayment", "loan_id":loan_id, "account_id":"bank",
        "date":"2026-11-15", "principal":principal, "interest":interest,
        "fee":fee, "category_id":category
    })
}

fn purchase_entry(
    account: &str,
    category: &str,
    amount: i64,
    date: &str,
    first_due: &str,
    periods: u16,
) -> Value {
    json!({
        "kind":"purchase", "name":"Compra", "account_id":account,
        "category_id":category, "amount":amount, "date":date,
        "first_due":first_due, "periods":periods
    })
}

fn payment_change(id: &str, account: &str, card: &str, amount: i64, date: &str) -> Value {
    json!({
        "request_id":format!("req-{id}"), "id":id, "expected_version":0,
        "deleted":false, "kind":"payment", "account_id":account,
        "card_account_id":card, "amount":amount, "date":date
    })
}

fn payment_edit(id: &str, account: &str, card: &str, amount: i64, version: i64) -> Value {
    json!({
        "request_id":format!("req-{id}-{version}"), "id":id, "expected_version":version,
        "deleted":false, "kind":"payment", "account_id":account,
        "card_account_id":card, "amount":amount, "date":"2026-11-15"
    })
}

fn allocate_entry(operation_id: &str, version: i64, month: &str, amount: i64) -> Value {
    json!({
        "kind":"allocate", "operation_id":operation_id,
        "expected_version":version, "month":month, "amount":amount
    })
}

fn refund_change(id: &str, account: &str, category: &str, original: &str, amount: i64) -> Value {
    json!({
        "request_id":format!("req-{id}"), "id":id, "expected_version":0,
        "deleted":false, "kind":"refund", "account_id":account,
        "category_id":category, "original_operation_id":original,
        "amount":amount, "date":"2026-11-05"
    })
}

fn installment_interest(loans: &Value, id: &str) -> i64 {
    loans
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == id)
        .unwrap()["installments"][0]["interest"]
        .as_i64()
        .unwrap()
}

async fn create_loan(f: &Fixture, session: &Session, principal: i64) -> String {
    let (status, id) = f
        .send_new(session, loan(principal, "bank", "2026-11-01", 0))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    id
}

#[tokio::test]
async fn b01_disbursement_creates_cash_and_debt_without_net_worth_change() {
    let f = Fixture::new().await;
    f.account(&f.admin.id, "bank", "bank", 0);
    let loan_id = create_loan(&f, &f.admin, 1000).await;

    assert_eq!(f.balance(&f.admin.id, "bank"), 1000);
    let principal: i64 =
        f.db.0
            .lock()
            .unwrap()
            .query_row(
                "SELECT principal FROM credit_loans WHERE user_id=?1 AND id=?2",
                params![f.admin.id, loan_id],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(principal, 1000);
    assert_eq!(
        f.scalar(
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND kind='loan_draw'",
            &f.admin.id
        ),
        1
    );
    assert_eq!(
        f.scalar(
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND kind IN ('income','expense')",
            &f.admin.id
        ),
        0
    );
    assert_eq!(f.postings(&f.admin.id).len(), 1);

    let report = f.report(&f.admin).await;
    assert_eq!(report["balances"]["bank"], json!("1000"));
    assert_eq!(report["net_worth"], json!("0"));
    assert_eq!(report["income"], json!("0"));
    assert_eq!(report["expense"], json!("0"));
    assert_eq!(report["net"], json!("0"));
    assert_eq!(report["cash_flow"], json!("1000"));
    assert_eq!(report["loan_debt"], json!("1000"));
    let portfolio = f.portfolio(&f.admin).await;
    assert_eq!(portfolio["loans"][0]["outstanding"], json!(1000));
    f.integrity();
}

#[tokio::test]
async fn b02_partial_repayment_separates_capital_interest_and_fee() {
    let f = Fixture::new().await;
    f.account(&f.admin.id, "bank", "bank", 0);
    f.category(&f.admin.id, "interest");
    let loan_id = create_loan(&f, &f.admin, 1000).await;
    let (status, payment_id) = f
        .send_new(&f.admin, repayment(&loan_id, 200, 20, 1, Some("interest")))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert_eq!(f.balance(&f.admin.id, "bank"), 779);
    let paid: (i64, i64, i64) = {
        let conn = f.db.0.lock().unwrap();
        conn.query_row(
            "SELECT principal,interest,fee FROM credit_repayments WHERE user_id=?1 AND id=?2",
            params![f.admin.id, payment_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    };
    assert_eq!(paid, (200, 20, 1));
    assert_eq!(
        f.postings(&f.admin.id)
            .iter()
            .map(|(_, _, d)| *d)
            .sum::<i64>(),
        779
    );
    assert_eq!(f.postings(&f.admin.id).len(), 4);

    let report = f.report(&f.admin).await;
    assert_eq!(report["expense"], json!("21"));
    assert_eq!(report["net"], json!("-21"));
    assert_eq!(report["net_worth"], json!("-21"));
    assert_eq!(report["cash_flow"], json!("779"));
    assert_eq!(report["loan_principal"], json!("200"));
    assert_eq!(report["loan_interest"], json!("20"));
    assert_eq!(report["loan_fees"], json!("1"));
    let portfolio = f.portfolio(&f.admin).await;
    assert_eq!(portfolio["loans"][0]["outstanding"], json!(800));
    assert_eq!(
        portfolio["loans"][0]["payments"].as_array().unwrap().len(),
        1
    );
    f.integrity();
}

#[tokio::test]
async fn b03_extra_payment_settles_without_changing_the_contract() {
    let f = Fixture::new().await;
    f.account(&f.admin.id, "bank", "bank", 0);
    let loan_id = create_loan(&f, &f.admin, 1000).await;
    for amount in [300, 300, 300, 100] {
        let (status, _) = f
            .send_new(&f.admin, repayment(&loan_id, amount, 0, 0, None))
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    let (status, _) = f
        .send_new(&f.admin, repayment(&loan_id, 1, 0, 0, None))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let portfolio = f.portfolio(&f.admin).await;
    assert_eq!(portfolio["loans"][0]["outstanding"], json!(0));
    assert_eq!(
        portfolio["loans"][0]["payments"].as_array().unwrap().len(),
        4
    );
    let periods: i64 =
        f.db.0
            .lock()
            .unwrap()
            .query_row(
                "SELECT periods FROM credit_loans WHERE user_id=?1 AND id=?2",
                params![f.admin.id, loan_id],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(periods, 2);
    let installments = portfolio["loans"][0]["installments"].as_array().unwrap();
    assert_eq!(installments.len(), 2);
    assert_eq!(
        installments
            .iter()
            .map(|i| i["principal"].as_i64().unwrap())
            .sum::<i64>(),
        1000
    );
    assert_eq!(
        f.scalar(
            "SELECT COALESCE(SUM(principal),0) FROM credit_repayments WHERE user_id=?",
            &f.admin.id
        ),
        1000
    );
    assert_eq!(
        f.scalar(
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND kind='expense'",
            &f.admin.id
        ),
        0
    );
    let report = f.report(&f.admin).await;
    assert_eq!(report["net_worth"], json!("0"));
    assert_eq!(report["cash_flow"], json!("0"));
    assert_eq!(report["expense"], json!("0"));
    f.integrity();
}

#[tokio::test]
async fn b04_cancellation_reverses_every_component_once() {
    let f = Fixture::new().await;
    f.account(&f.admin.id, "bank", "bank", 0);
    f.category(&f.admin.id, "interest");
    let loan_id = create_loan(&f, &f.admin, 1000).await;
    let (status, payment_id) = f
        .send_new(&f.admin, repayment(&loan_id, 200, 20, 1, Some("interest")))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(f.balance(&f.admin.id, "bank"), 779);

    let cancel = json!({"kind":"cancel_repayment","repayment_id":payment_id});
    let cancel_id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        f.send(&f.admin, cancel.clone(), &cancel_id).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.send(&f.admin, cancel.clone(), &cancel_id).await,
        StatusCode::NO_CONTENT
    );

    assert_eq!(f.balance(&f.admin.id, "bank"), 1000);
    assert_eq!(
        f.scalar(
            "SELECT COUNT(*) FROM credit_repayments WHERE user_id=?",
            &f.admin.id
        ),
        0
    );
    let (deleted, history, previous) = {
        let conn = f.db.0.lock().unwrap();
        let deleted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM finance_operations WHERE user_id=?1 AND deleted=1",
                params![f.admin.id],
                |r| r.get(0),
            )
            .unwrap();
        let history: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM finance_operation_history WHERE user_id=?1",
                params![f.admin.id],
                |r| r.get(0),
            )
            .unwrap();
        let previous: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM finance_operation_history WHERE user_id=?1 AND previous IS NOT NULL",
                params![f.admin.id],
                |r| r.get(0),
            )
            .unwrap();
        (deleted, history, previous)
    };
    assert_eq!(deleted, 3);
    assert_eq!(history, 7);
    assert_eq!(previous, 3);

    let report = f.report(&f.admin).await;
    assert_eq!(report["expense"], json!("0"));
    assert_eq!(report["net"], json!("0"));
    assert_eq!(report["net_worth"], json!("0"));
    assert_eq!(report["cash_flow"], json!("1000"));
    assert_eq!(report["loan_principal"], json!("0"));
    assert_eq!(report["loan_interest"], json!("0"));
    assert_eq!(report["loan_fees"], json!("0"));
    assert_eq!(
        f.portfolio(&f.admin).await["loans"][0]["outstanding"],
        json!(1000)
    );
    f.integrity();
}

#[tokio::test]
async fn b05_rejections_leave_no_partial_writes_and_do_not_consume_request_ids() {
    let f = Fixture::new().await;
    f.account(&f.admin.id, "bank", "bank", 0);
    f.category(&f.admin.id, "interest");
    let loan_id = create_loan(&f, &f.admin, 1000).await;
    let other = f.new_user("other").await;
    f.account(&other.id, "bank", "bank", 0);
    let other_loan = create_loan(&f, &other, 500).await;

    let cases = vec![
        (
            repayment(&loan_id, 100, 10, 0, None),
            StatusCode::BAD_REQUEST,
        ),
        (
            repayment(&loan_id, 100, 0, 1, None),
            StatusCode::BAD_REQUEST,
        ),
        (
            repayment(&loan_id, 1001, 0, 0, None),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind":"repayment","loan_id":loan_id,"account_id":"bank","date":"2026-10-31","principal":10,"interest":0,"fee":0,"category_id":null}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind":"repayment","loan_id":loan_id,"account_id":"card","date":"2026-11-15","principal":10,"interest":0,"fee":0,"category_id":null}),
            StatusCode::BAD_REQUEST,
        ),
        (
            repayment(&loan_id, 0, 0, 0, Some("interest")),
            StatusCode::BAD_REQUEST,
        ),
        (
            repayment(&loan_id, 9_007_199_254_740_991, 1, 0, Some("interest")),
            StatusCode::BAD_REQUEST,
        ),
        (
            repayment(&uuid::Uuid::new_v4().to_string(), 10, 0, 0, None),
            StatusCode::NOT_FOUND,
        ),
        (
            repayment(&other_loan, 10, 0, 0, None),
            StatusCode::NOT_FOUND,
        ),
    ];
    for (entry, expected) in cases {
        let (status, request_id) = f.send_new(&f.admin, entry).await;
        assert_eq!(status, expected, "unexpected status for {request_id}");
        assert_eq!(
            f.scalar(
                "SELECT COUNT(*) FROM credit_requests WHERE user_id=?",
                &f.admin.id
            ),
            1,
            "rejected request must not be recorded"
        );
        assert_eq!(f.balance(&f.admin.id, "bank"), 1000);
        assert_eq!(f.postings(&f.admin.id).len(), 1);
    }

    // A request id used by a rejected transaction is still available.
    let id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        f.send(
            &f.admin,
            repayment(&loan_id, 100, 10, 1, Some("interest")),
            &id
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(f.balance(&f.admin.id, "bank"), 889);
    f.integrity();
}

#[tokio::test]
async fn b06_replay_is_idempotent_and_changed_payload_conflicts() {
    let f = Fixture::new().await;
    f.account(&f.admin.id, "bank", "bank", 0);
    f.category(&f.admin.id, "interest");
    let loan_id = create_loan(&f, &f.admin, 1000).await;
    let id = uuid::Uuid::new_v4().to_string();
    let entry = repayment(&loan_id, 200, 20, 1, Some("interest"));

    assert_eq!(
        f.send(&f.admin, entry.clone(), &id).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.send(&f.admin, entry.clone(), &id).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.send(
            &f.admin,
            repayment(&loan_id, 201, 20, 1, Some("interest")),
            &id
        )
        .await,
        StatusCode::CONFLICT
    );
    assert_eq!(f.balance(&f.admin.id, "bank"), 779);
    assert_eq!(
        f.scalar(
            "SELECT COUNT(*) FROM credit_repayments WHERE user_id=?",
            &f.admin.id
        ),
        1
    );
    let requests: i64 =
        f.db.0
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM credit_requests WHERE user_id=?1 AND id=?2",
                params![f.admin.id, id],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(requests, 1);
    assert_eq!(
        f.portfolio(&f.admin).await["loans"][0]["outstanding"],
        json!(800)
    );
    f.integrity();
}

#[tokio::test]
async fn c01_card_payment_settles_debt_without_expense() {
    let f = Fixture::new().await;
    f.account(&f.admin.id, "bank", "bank", 1000);
    f.account(&f.admin.id, "card", "card", 300);
    let before = f.report(&f.admin).await;
    assert_eq!(before["net_worth"], json!("700"));

    assert_eq!(
        f.operation(
            &f.admin,
            payment_change("pay-card", "bank", "card", 300, "2026-11-15")
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(f.balance(&f.admin.id, "bank"), 700);
    assert_eq!(f.card_balance(&f.admin.id, "card"), 0);
    assert_eq!(
        f.postings(&f.admin.id),
        vec![
            ("pay-card".into(), "bank".into(), -300),
            ("pay-card".into(), "card".into(), -300),
        ]
    );
    let report = f.report(&f.admin).await;
    assert_eq!(report["balances"]["bank"], json!("700"));
    assert_eq!(report["balances"]["card"], json!("0"));
    assert_eq!(report["net_worth"], json!("700"));
    assert_eq!(report["income"], json!("0"));
    assert_eq!(report["expense"], json!("0"));
    assert_eq!(report["net"], json!("0"));
    assert_eq!(report["cash_flow"], json!("-300"));
    f.integrity();
}

#[tokio::test]
async fn c02_purchase_then_payment_counts_the_expense_once() {
    let f = Fixture::new().await;
    f.account(&f.admin.id, "bank", "bank", 1000);
    f.account(&f.admin.id, "card", "card", 0);
    f.category(&f.admin.id, "goods");
    let (status, request) = f
        .send_new(
            &f.admin,
            purchase_entry("card", "goods", 300, "2026-11-02", "2026-11-20", 1),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let purchase_id = format!("loan:{request}:purchase");
    assert_eq!(f.card_balance(&f.admin.id, "card"), 300);
    let after_purchase = f.report(&f.admin).await;
    assert_eq!(after_purchase["net_worth"], json!("700"));
    assert_eq!(after_purchase["expense"], json!("300"));
    assert_eq!(after_purchase["net"], json!("-300"));
    assert_eq!(after_purchase["cash_flow"], json!("0"));
    assert_eq!(
        f.scalar(
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=?",
            &f.admin.id
        ),
        1
    );

    assert_eq!(
        f.operation(
            &f.admin,
            payment_change("pay-buy", "bank", "card", 300, "2026-11-15")
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(f.balance(&f.admin.id, "bank"), 700);
    assert_eq!(f.card_balance(&f.admin.id, "card"), 0);
    assert_eq!(
        f.postings(&f.admin.id)
            .iter()
            .map(|(_, _, d)| *d)
            .sum::<i64>(),
        -300
    );
    let report = f.report(&f.admin).await;
    assert_eq!(report["net_worth"], json!("700"));
    assert_eq!(report["expense"], json!("300"));
    assert_eq!(report["net"], json!("-300"));
    assert_eq!(report["cash_flow"], json!("-300"));
    let portfolio = f.portfolio(&f.admin).await;
    assert_eq!(
        portfolio["purchases"][0]["operation_id"],
        json!(purchase_id)
    );
    assert_eq!(
        portfolio["purchases"][0]["installments"][0]["principal"],
        json!(300)
    );
    f.integrity();
}

#[tokio::test]
async fn c03_installments_are_projected_not_posted() {
    let f = Fixture::new().await;
    f.account(&f.admin.id, "bank", "bank", 1000);
    f.account(&f.admin.id, "card", "card", 0);
    f.category(&f.admin.id, "goods");
    let (status, request) = f
        .send_new(
            &f.admin,
            purchase_entry("card", "goods", 300, "2026-01-02", "2026-01-15", 3),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let portfolio = f.portfolio(&f.admin).await;
    let installments = portfolio["purchases"][0]["installments"]
        .as_array()
        .unwrap();
    assert_eq!(installments.len(), 3);
    assert_eq!(
        installments
            .iter()
            .map(|i| (
                i["date"].as_str().unwrap(),
                i["principal"].as_i64().unwrap()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("2026-01-15", 100),
            ("2026-02-15", 100),
            ("2026-03-15", 100),
        ]
    );
    assert_eq!(
        installments
            .iter()
            .map(|i| i["principal"].as_i64().unwrap())
            .sum::<i64>(),
        300
    );
    assert_eq!(f.card_balance(&f.admin.id, "card"), 300);
    assert_eq!(
        f.scalar(
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=?",
            &f.admin.id
        ),
        1,
        "no accounting operation per projected installment"
    );
    assert_eq!(
        f.scalar(
            "SELECT COUNT(*) FROM credit_purchases WHERE user_id=?",
            &f.admin.id
        ),
        1
    );

    assert_eq!(
        f.operation(
            &f.admin,
            payment_change("pay-cuota", "bank", "card", 100, "2026-03-10")
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(f.card_balance(&f.admin.id, "card"), 200);
    let portfolio = f.portfolio(&f.admin).await;
    assert_eq!(
        portfolio["purchases"][0]["installments"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(f.card_balance(&f.admin.id, "card"), 200);
    let _ = request;
    f.integrity();
}

#[tokio::test]
async fn c04_cancel_purchase_reverts_and_respects_refunds() {
    let f = Fixture::new().await;
    f.account(&f.admin.id, "bank", "bank", 1000);
    f.account(&f.admin.id, "card", "card", 0);
    f.category(&f.admin.id, "goods");
    let (status, request) = f
        .send_new(
            &f.admin,
            purchase_entry("card", "goods", 300, "2026-11-02", "2026-11-20", 1),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let purchase_id = format!("loan:{request}:purchase");
    let cancel = json!({"kind":"cancel_purchase","operation_id":purchase_id});
    let (status, cancel_id) = f.send_new(&f.admin, cancel.clone()).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        f.send(&f.admin, cancel.clone(), &cancel_id).await,
        StatusCode::NO_CONTENT
    );
    let foreign = f.new_user("foreign").await;
    let (status, _) = f.send_new(&foreign, cancel).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    assert_eq!(f.card_balance(&f.admin.id, "card"), 0);
    let (deleted, history) = {
        let conn = f.db.0.lock().unwrap();
        let deleted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM finance_operations WHERE user_id=?1 AND deleted=1",
                params![f.admin.id],
                |r| r.get(0),
            )
            .unwrap();
        let history: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM finance_operation_history WHERE user_id=?1",
                params![f.admin.id],
                |r| r.get(0),
            )
            .unwrap();
        (deleted, history)
    };
    assert_eq!(deleted, 1);
    assert_eq!(history, 2);
    assert_eq!(
        f.scalar(
            "SELECT COUNT(*) FROM credit_purchases WHERE user_id=?",
            &f.admin.id
        ),
        0
    );
    assert!(
        f.portfolio(&f.admin).await["purchases"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let report = f.report(&f.admin).await;
    assert_eq!(report["expense"], json!("0"));
    assert_eq!(report["net_worth"], json!("1000"));
    f.integrity();

    // A linked refund blocks cancellation (existing refund rule, unchanged).
    let g = Fixture::new().await;
    g.account(&g.admin.id, "bank", "bank", 1000);
    g.account(&g.admin.id, "card", "card", 0);
    g.category(&g.admin.id, "goods");
    let (status, request) = g
        .send_new(
            &g.admin,
            purchase_entry("card", "goods", 300, "2026-11-02", "2026-11-20", 1),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let purchase_id = format!("loan:{request}:purchase");
    assert_eq!(
        g.operation(
            &g.admin,
            refund_change("refund", "card", "goods", &purchase_id, 50)
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(g.card_balance(&g.admin.id, "card"), 250);
    let (status, _) = g
        .send_new(
            &g.admin,
            json!({"kind":"cancel_purchase","operation_id":purchase_id}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(g.report(&g.admin).await["expense"], json!("250"));
    g.integrity();
}

#[tokio::test]
async fn c05_monthly_allocations_follow_payment_version() {
    let f = Fixture::new().await;
    f.account(&f.admin.id, "bank", "bank", 1000);
    f.account(&f.admin.id, "card", "card", 0);
    assert_eq!(
        f.operation(
            &f.admin,
            payment_change("pay-a", "bank", "card", 100, "2026-11-15")
        )
        .await,
        StatusCode::OK
    );
    for (month, amount) in [("2026-01", 70), ("2026-02", 30)] {
        assert_eq!(
            f.send_new(&f.admin, allocate_entry("pay-a", 1, month, amount))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
    }
    assert_eq!(
        f.send_new(&f.admin, allocate_entry("pay-a", 1, "2026-02", 40))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.send_new(&f.admin, allocate_entry("pay-a", 0, "2026-02", 30))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let allocations = f.portfolio(&f.admin).await["allocations"].clone();
    assert_eq!(
        allocations
            .as_array()
            .unwrap()
            .iter()
            .map(|a| (a["month"].as_str().unwrap(), a["amount"].as_i64().unwrap()))
            .collect::<Vec<_>>(),
        vec![("2026-01", 70), ("2026-02", 30)]
    );

    assert_eq!(
        f.operation(
            &f.admin,
            payment_change("pay-b", "bank", "card", 50, "2026-11-16")
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        f.send_new(&f.admin, allocate_entry("pay-b", 1, "2026-01", 50))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.operation(&f.admin, payment_edit("pay-a", "bank", "card", 60, 1))
            .await,
        StatusCode::OK
    );
    let remaining = f.scalar(
        "SELECT COUNT(*) FROM credit_allocations WHERE user_id=?",
        &f.admin.id,
    );
    assert_eq!(remaining, 1);
    let allocations = f.portfolio(&f.admin).await["allocations"].clone();
    assert_eq!(allocations.as_array().unwrap().len(), 1);
    assert_eq!(allocations[0]["operation_id"], json!("pay-b"));
    assert_eq!(allocations[0]["amount"], json!(50));
    f.integrity();
}

#[tokio::test]
async fn c06_initial_card_balance_sign_is_unambiguous_in_the_backend() {
    let f = Fixture::new().await;
    f.account(&f.admin.id, "bank", "bank", 1000);
    f.account(&f.admin.id, "debt", "card", 300);
    f.account(&f.admin.id, "credit", "card", -300);
    f.details(&f.admin.id, "debt", Some(20), Some(5), Some("2026-11-05"));
    f.details(&f.admin.id, "credit", Some(20), Some(5), Some("2026-11-05"));
    let report = f.report(&f.admin).await;
    assert_eq!(report["balances"]["debt"], json!("300"));
    assert_eq!(report["balances"]["credit"], json!("-300"));
    // Backend: a positive card balance is a debt, a negative one is a credit.
    assert_eq!(report["net_worth"], json!("1000"));
    f.integrity();
}

#[tokio::test]
async fn c07_card_rejections_are_atomic_and_idempotent() {
    let f = Fixture::new().await;
    f.account(&f.admin.id, "bank", "bank", 1000);
    f.account(&f.admin.id, "card", "card", 0);
    f.category(&f.admin.id, "goods");
    let (status, request) = f
        .send_new(
            &f.admin,
            purchase_entry("card", "goods", 300, "2026-11-02", "2026-11-20", 1),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let purchase = purchase_entry("card", "goods", 300, "2026-11-02", "2026-11-20", 1);
    assert_eq!(
        f.send(&f.admin, purchase.clone(), &request).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.send(
            &f.admin,
            purchase_entry("card", "goods", 999, "2026-11-02", "2026-11-20", 1),
            &request
        )
        .await,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.scalar(
            "SELECT COUNT(*) FROM credit_purchases WHERE user_id=?",
            &f.admin.id
        ),
        1
    );

    for (change, expected) in [
        (
            payment_change("p-missing", "ghost", "card", 10, "2026-11-15"),
            StatusCode::BAD_REQUEST,
        ),
        (
            payment_change("p-no-card", "bank", "bank", 10, "2026-11-15"),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({
                "request_id":"alloc-over", "id":"pay-a", "expected_version":0,
                "deleted":false, "kind":"payment", "account_id":"bank",
                "card_account_id":"card", "amount":10, "date":"2026-11-15"
            }),
            StatusCode::OK,
        ),
    ] {
        assert_eq!(f.operation(&f.admin, change).await, expected);
    }
    assert_eq!(
        f.send_new(&f.admin, allocate_entry("pay-a", 1, "2026-01", 11))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.send_new(
            &f.admin,
            json!({"kind":"cancel_purchase","operation_id":"missing"})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(f.card_balance(&f.admin.id, "card"), 290);
    assert_eq!(
        f.scalar(
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=?",
            &f.admin.id
        ),
        2
    );
    f.integrity();
}

#[tokio::test]
async fn d01_backend_matches_the_shared_projection_fixture() {
    let f = Fixture::new().await;
    let fixture: Value = serde_json::from_str(include_str!("fixtures/f92d.json")).unwrap();
    let data = &fixture["d01"]["loan"];
    f.account(&f.admin.id, "bank", "bank", 0);
    f.category(&f.admin.id, "interest");
    let entry = json!({
        "kind":"loan", "name":"Préstamo", "account_id":"bank",
        "principal":data["principal"], "date":data["date"],
        "first_due":data["first_due"], "periods":data["periods"],
        "method":data["method"], "rate_bps":data["rate_bps"],
        "rate_type":data["rate_type"]
    });
    let (status, loan_id) = f.send_new(&f.admin, entry).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let portfolio = f.portfolio(&f.admin).await;
    assert_eq!(portfolio["loans"][0]["installments"], data["installments"]);
    let version: u8 =
        f.db.0
            .lock()
            .unwrap()
            .query_row(
                "SELECT schedule_version FROM credit_loans WHERE user_id=?1 AND id=?2",
                params![f.admin.id, loan_id],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(version, 2);

    let first = &data["first_payment"];
    let payment = json!({
        "kind":"repayment", "loan_id":loan_id, "account_id":"bank",
        "date":first["date"], "principal":first["principal"],
        "interest":first["interest"], "fee":first["fee"], "category_id":"interest"
    });
    let (status, _) = f.send_new(&f.admin, payment).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        f.portfolio(&f.admin).await["loans"][0]["outstanding"],
        data["outstanding_after_first"]
    );

    let report = f.report_for(&f.admin, "2026-01").await;
    for (key, expected) in data["report_2026_01"].as_object().unwrap() {
        assert_eq!(&report[key], expected, "report field {key}");
    }
    f.integrity();
}

#[tokio::test]
async fn d05_backend_card_signs_match_the_shared_fixture() {
    let f = Fixture::new().await;
    let fixture: Value = serde_json::from_str(include_str!("fixtures/f92d.json")).unwrap();
    let cards = &fixture["d05"];
    f.account(&f.admin.id, "bank", "bank", 0);
    for (id, key) in [("debt", "debt"), ("credit", "credit"), ("zero", "zero")] {
        f.account(
            &f.admin.id,
            id,
            "card",
            cards[key]["initial_balance"].as_i64().unwrap(),
        );
    }
    f.details(
        &f.admin.id,
        "debt",
        Some(20),
        Some(10),
        cards["debt"]["cutoff_date"].as_str(),
    );
    f.details(
        &f.admin.id,
        "credit",
        Some(20),
        Some(10),
        cards["credit"]["cutoff_date"].as_str(),
    );
    f.details(
        &f.admin.id,
        "zero",
        Some(20),
        Some(10),
        cards["zero"]["cutoff_date"].as_str(),
    );
    let report = f.report(&f.admin).await;
    for (id, key) in [("debt", "debt"), ("credit", "credit")] {
        assert_eq!(
            report["balances"][id],
            json!(cards[key]["backend_balance"].as_i64().unwrap().to_string()),
            "{id} balance"
        );
    }
    assert_eq!(report["net_worth"], json!("0"));
    f.integrity();
}

#[tokio::test]
async fn b07_v1_and_v2_loans_both_accept_payments_and_cancellations() {
    let f = Fixture::new().await;
    f.account(&f.admin.id, "bank1", "bank", 0);
    f.account(&f.admin.id, "bank2", "bank", 0);
    let big = 7_000_000_000_000_003i64;

    let (status, v2_id) = f
        .send_new(&f.admin, loan(big, "bank2", "2026-11-01", 7000))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, v1_id) = f
        .send_new(&f.admin, loan(big, "bank1", "2026-11-01", 7000))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    f.db.0
        .lock()
        .unwrap()
        .execute(
            "UPDATE credit_loans SET schedule_version=1 WHERE user_id=?1 AND id=?2",
            params![f.admin.id, v1_id],
        )
        .unwrap();

    let portfolio = f.portfolio(&f.admin).await;
    assert_eq!(portfolio["loans"].as_array().unwrap().len(), 2);
    assert_eq!(
        installment_interest(&portfolio["loans"], &v1_id),
        408_333_333_333_333
    );
    assert_eq!(
        installment_interest(&portfolio["loans"], &v2_id),
        408_333_333_333_334
    );

    for (id, account) in [(&v1_id, "bank1"), (&v2_id, "bank2")] {
        let entry = json!({
            "kind":"repayment", "loan_id":id, "account_id":account,
            "date":"2026-11-15", "principal":100, "interest":0,
            "fee":0, "category_id":null
        });
        let (status, payment_id) = f.send_new(&f.admin, entry).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let cancel = json!({"kind":"cancel_repayment","repayment_id":payment_id});
        let (status, _) = f.send_new(&f.admin, cancel).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    let mut versions: Vec<i64> = {
        let conn = f.db.0.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT schedule_version FROM credit_loans WHERE user_id=? ORDER BY id")
            .unwrap();
        stmt.query_map([&f.admin.id], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    versions.sort_unstable();
    assert_eq!(versions, vec![1, 2]);
    let portfolio = f.portfolio(&f.admin).await;
    assert_eq!(
        installment_interest(&portfolio["loans"], &v1_id),
        408_333_333_333_333
    );
    assert_eq!(
        installment_interest(&portfolio["loans"], &v2_id),
        408_333_333_333_334
    );
    assert_eq!(
        portfolio["loans"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["payments"].as_array().unwrap().len())
            .sum::<usize>(),
        0
    );
    f.integrity();
}
