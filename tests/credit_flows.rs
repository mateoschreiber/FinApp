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
        let res = self
            .request(
                session,
                "GET",
                &format!("/api/planning/month/{MONTH}"),
                json!({}),
            )
            .await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&body).unwrap()
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
