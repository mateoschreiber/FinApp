//! F9.1B: independent financial oracles. Every expected number below is a literal
//! computed by hand; production report()/balances_in() are never the oracle.
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
};
use finapp::{
    app,
    db::Database,
    finance::{self, Account, Category, Change, Outcome},
};
use http_body_util::BodyExt;
use rusqlite::params;
use serde_json::{Value, json};
use tower::ServiceExt;

const ORIGIN: &str = "https://finanzas.example";
const PASSWORD: &str = "contraseña temporal sintética";

struct Fixture {
    db: Database,
    app: Router,
    user: String,
    cookie: String,
}

impl Fixture {
    async fn new() -> Self {
        let db = Database::open_memory().unwrap();
        db.bootstrap_admin("admin", PASSWORD).unwrap();
        let app = app(db.clone(), ORIGIN.to_owned());
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/login")
                    .header("origin", ORIGIN)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"username":"admin","password":PASSWORD}).to_string(),
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
        let user = serde_json::from_slice::<Value>(&body).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        Self {
            db,
            app,
            user,
            cookie,
        }
    }

    fn account(&self, id: &str, kind: &str, initial: i64) {
        finance::save_account(
            &self.db,
            &self.user,
            &Account {
                id: id.into(),
                name: id.into(),
                kind: kind.into(),
                initial_balance: initial,
            },
        )
        .unwrap();
    }

    fn category(&self, id: &str, kind: &str) {
        finance::save_category(
            &self.db,
            &self.user,
            &Category {
                id: id.into(),
                name: id.into(),
                kind: kind.into(),
            },
        )
        .unwrap();
    }

    // Fixture-only: dates of cut live in the catalogue; insert directly so the
    // oracle does not depend on the catalog::save code path being tested.
    fn cutoff(&self, account: &str, date: &str) {
        self.db
            .0
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO catalog_details(user_id,entity,id,archived,institution,notes,cutoff_date)
                 VALUES(?1,'account',?2,0,'','',?3)",
                params![self.user, account, date],
            )
            .unwrap();
    }

    fn apply(&self, change: &Change) -> Result<Outcome, finance::Error> {
        finance::apply(&self.db, &self.user, change)
    }

    // Oracle: initial balance + raw postings, never balances_in().
    fn balance_as_of(&self, account: &str, end: Option<&str>) -> i64 {
        let conn = self.db.0.lock().unwrap();
        let initial: i64 = conn
            .query_row(
                "SELECT initial_balance FROM finance_accounts WHERE user_id=?1 AND id=?2",
                params![self.user, account],
                |r| r.get(0),
            )
            .unwrap();
        let delta: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(delta),0) FROM finance_balance_postings
                 WHERE user_id=?1 AND account_id=?2 AND (?3 IS NULL OR date<=?3)",
                params![self.user, account, end],
                |r| r.get(0),
            )
            .unwrap();
        initial + delta
    }

    fn count(&self, sql: &str) -> i64 {
        self.db
            .0
            .lock()
            .unwrap()
            .query_row(sql, [&self.user], |r| r.get(0))
            .unwrap()
    }

    fn postings(&self) -> Vec<(String, String, i64)> {
        let conn = self.db.0.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id,account_id,delta FROM finance_postings WHERE user_id=?1 ORDER BY id,account_id",
            )
            .unwrap();
        stmt.query_map([&self.user], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    async fn monthly(&self, month: &str) -> Value {
        let res: Response = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/planning/month/{month}"))
                    .header("cookie", &self.cookie)
                    .header("x-finapp-user", &self.user)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&body).unwrap()
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

fn change(id: &str, request: &str, kind: &str, account: &str, amount: i64, date: &str) -> Change {
    Change {
        request_id: request.into(),
        id: id.into(),
        expected_version: 0,
        deleted: false,
        kind: Some(kind.into()),
        account_id: Some(account.into()),
        category_id: None,
        card_account_id: None,
        destination_account_id: None,
        original_operation_id: None,
        amount: Some(amount),
        date: Some(date.into()),
        description: None,
        notes: None,
    }
}

#[tokio::test]
async fn scenario_a_income_and_expense() {
    let f = Fixture::new().await;
    f.account("bank", "bank", 1000);
    f.account("cash", "cash", 200);
    f.category("salary", "income");
    f.category("food", "expense");

    let mut income = change("a-income", "a-income", "income", "bank", 500, "2026-11-05");
    income.category_id = Some("salary".into());
    assert!(matches!(f.apply(&income), Ok(Outcome::Applied(_))));
    let mut expense = change(
        "a-expense",
        "a-expense",
        "expense",
        "cash",
        300,
        "2026-11-10",
    );
    expense.category_id = Some("food".into());
    // Negative balances are allowed by the current policy; this is a valid fixture.
    assert!(matches!(f.apply(&expense), Ok(Outcome::Applied(_))));

    assert_eq!(f.balance_as_of("bank", None), 1500);
    assert_eq!(f.balance_as_of("cash", None), -100);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM finance_operations WHERE user_id=?"),
        2
    );

    let report = f.monthly("2026-11").await;
    assert_eq!(report["balances"]["bank"], json!("1500"));
    assert_eq!(report["balances"]["cash"], json!("-100"));
    assert_eq!(report["net_worth"], json!("1400"));
    assert_eq!(report["income"], json!("500"));
    assert_eq!(report["expense"], json!("300"));
    assert_eq!(report["net"], json!("200"));
    assert_eq!(report["cash_flow"], json!("200"));
    assert_eq!(
        f.postings(),
        vec![
            ("a-expense".into(), "cash".into(), -300),
            ("a-income".into(), "bank".into(), 500),
        ]
    );
    f.integrity();
}

#[tokio::test]
async fn scenario_b_transfer_keeps_net_worth() {
    let f = Fixture::new().await;
    f.account("a", "bank", 1000);
    f.account("b", "bank", 0);

    let mut transfer = change(
        "b-transfer",
        "b-transfer",
        "transfer",
        "a",
        400,
        "2026-11-15",
    );
    transfer.destination_account_id = Some("b".into());
    assert!(matches!(f.apply(&transfer), Ok(Outcome::Applied(_))));

    assert_eq!(f.balance_as_of("a", None), 600);
    assert_eq!(f.balance_as_of("b", None), 400);
    assert_eq!(
        f.postings(),
        vec![
            ("b-transfer".into(), "a".into(), -400),
            ("b-transfer".into(), "b".into(), 400),
        ]
    );

    let report = f.monthly("2026-11").await;
    assert_eq!(report["income"], json!("0"));
    assert_eq!(report["expense"], json!("0"));
    assert_eq!(report["net"], json!("0"));
    assert_eq!(report["cash_flow"], json!("0"));
    assert_eq!(report["net_worth"], json!("1000"));
    f.integrity();
}

#[tokio::test]
async fn scenario_c_refund_is_capped() {
    let f = Fixture::new().await;
    f.account("bank", "bank", 0);
    f.account("cash", "cash", 0);
    f.category("food", "expense");

    let mut expense = change("c-exp", "c-exp", "expense", "bank", 500, "2026-11-02");
    expense.category_id = Some("food".into());
    f.apply(&expense).unwrap();
    let mut refund = change("c-ref", "c-ref", "refund", "bank", 200, "2026-11-12");
    refund.category_id = Some("food".into());
    refund.original_operation_id = Some("c-exp".into());
    f.apply(&refund).unwrap();

    assert_eq!(f.balance_as_of("bank", None), -300);
    let report = f.monthly("2026-11").await;
    assert_eq!(report["expense"], json!("300"));
    assert_eq!(report["net"], json!("-300"));
    assert_eq!(report["net_worth"], json!("-300"));
    assert_eq!(report["cash_flow"], json!("-300"));

    let mut over = change("c-over", "c-over", "refund", "bank", 301, "2026-11-13");
    over.category_id = Some("food".into());
    over.original_operation_id = Some("c-exp".into());
    assert!(matches!(f.apply(&over), Err(finance::Error::Invalid)));
    assert_eq!(f.balance_as_of("bank", None), -300);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM finance_operations WHERE user_id=?"),
        2
    );

    // Existing permissive rule: a refund may land on a different non-card account.
    let g = Fixture::new().await;
    g.account("bank", "bank", 0);
    g.account("cash", "cash", 0);
    g.category("food", "expense");
    let mut expense = change("c2-exp", "c2-exp", "expense", "bank", 500, "2026-11-02");
    expense.category_id = Some("food".into());
    g.apply(&expense).unwrap();
    let mut refund = change("c2-ref", "c2-ref", "refund", "cash", 200, "2026-11-12");
    refund.category_id = Some("food".into());
    refund.original_operation_id = Some("c2-exp".into());
    g.apply(&refund).unwrap();
    assert_eq!(g.balance_as_of("bank", None), -500);
    assert_eq!(g.balance_as_of("cash", None), 200);
    assert_eq!(g.monthly("2026-11").await["net_worth"], json!("-300"));
    f.integrity();
    g.integrity();
}

#[tokio::test]
async fn scenario_d_cancellation_reverses_exactly_once() {
    let f = Fixture::new().await;
    f.account("bank", "bank", 0);
    f.category("salary", "income");
    let mut income = change("d-inc", "d-create", "income", "bank", 700, "2026-11-03");
    income.category_id = Some("salary".into());
    assert!(matches!(f.apply(&income), Ok(Outcome::Applied(ref o)) if o.version == 1));

    let mut delete = change("d-inc", "d-delete", "income", "bank", 700, "2026-11-03");
    delete.request_id = "d-delete".into();
    delete.expected_version = 1;
    delete.deleted = true;
    assert!(matches!(f.apply(&delete), Ok(Outcome::Applied(ref o)) if o.version == 2 && o.deleted));

    assert_eq!(f.balance_as_of("bank", None), 0);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM finance_operations WHERE user_id=?"),
        1
    );
    assert_eq!(
        f.count("SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND id='d-inc' AND deleted=1 AND version=2"),
        1
    );
    assert_eq!(
        f.count("SELECT COUNT(*) FROM finance_operation_history WHERE user_id=? AND operation_id='d-inc'"),
        2
    );
    assert_eq!(
        f.count("SELECT COUNT(*) FROM finance_operation_history WHERE user_id=? AND previous IS NOT NULL"),
        1
    );

    // D1: identical replay returns the recorded response, no new writes.
    assert!(matches!(f.apply(&delete), Ok(Outcome::Applied(ref o)) if o.version == 2 && o.deleted));
    assert_eq!(
        f.count("SELECT COUNT(*) FROM finance_operation_history WHERE user_id=? AND operation_id='d-inc'"),
        2
    );
    // D2: a new request with a stale version conflicts without changing state.
    let mut stale = delete.clone();
    stale.request_id = "d-stale".into();
    assert!(matches!(f.apply(&stale), Ok(Outcome::Conflict(_))));
    assert_eq!(f.balance_as_of("bank", None), 0);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM finance_operations WHERE user_id=?"),
        1
    );

    let report = f.monthly("2026-11").await;
    assert_eq!(report["income"], json!("0"));
    assert_eq!(report["expense"], json!("0"));
    assert_eq!(report["net"], json!("0"));
    assert_eq!(report["cash_flow"], json!("0"));
    assert_eq!(report["net_worth"], json!("0"));
    f.integrity();
}

#[tokio::test]
async fn scenario_e_cutoff_dates() {
    let f = Fixture::new().await;
    f.account("cut15", "bank", 1000);
    f.account("cut01", "bank", 500);
    f.account("nocut", "bank", 100);
    f.category("food", "expense");
    f.category("salary", "income");
    f.cutoff("cut15", "2026-11-15");
    f.cutoff("cut01", "2026-11-01");

    let mut ops = vec![
        change("e1", "e1", "expense", "cut15", 100, "2026-11-10"),
        change("e2", "e2", "income", "cut15", 200, "2026-11-15"),
        change("e3", "e3", "expense", "cut15", 50, "2026-11-20"),
        change("e4", "e4", "expense", "cut01", 30, "2026-10-31"),
        change("e5", "e5", "income", "cut01", 40, "2026-11-01"),
        change("e6", "e6", "expense", "nocut", 10, "2026-11-05"),
    ];
    for op in &mut ops {
        op.category_id = Some(if op.kind.as_deref() == Some("income") {
            "salary".into()
        } else {
            "food".into()
        });
        f.apply(op).unwrap();
    }

    assert_eq!(f.balance_as_of("cut15", Some("2026-11-30")), 1150);
    assert_eq!(f.balance_as_of("cut01", Some("2026-11-30")), 540);
    assert_eq!(f.balance_as_of("nocut", Some("2026-11-30")), 90);

    let report = f.monthly("2026-11").await;
    assert_eq!(report["balances"]["cut15"], json!("1150"));
    assert_eq!(report["balances"]["cut01"], json!("540"));
    assert_eq!(report["balances"]["nocut"], json!("90"));
    assert_eq!(report["net_worth"], json!("1780"));
    assert_eq!(report["income"], json!("240"));
    assert_eq!(report["expense"], json!("160"));
    assert_eq!(report["net"], json!("80"));
    assert_eq!(report["cash_flow"], json!("80"));
    // The pre-cutoff movement stays visible even though it did not move the balance.
    assert!(
        report["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["id"] == "e1")
    );

    // Year rollover: cutoff on 1 Jan excludes 31 Dec, includes 1 Jan.
    f.account("year", "bank", 300);
    f.cutoff("year", "2027-01-01");
    let mut dec = change("e7", "e7", "expense", "year", 50, "2026-12-31");
    dec.category_id = Some("food".into());
    f.apply(&dec).unwrap();
    let mut jan = change("e8", "e8", "income", "year", 70, "2027-01-01");
    jan.category_id = Some("salary".into());
    f.apply(&jan).unwrap();
    assert_eq!(f.balance_as_of("year", Some("2026-12-31")), 300);
    assert_eq!(f.balance_as_of("year", Some("2027-01-31")), 370);
    f.integrity();
}

#[tokio::test]
async fn scenario_f_card_payment_settles_debt_without_expense() {
    let f = Fixture::new().await;
    f.account("bank", "bank", 1000);
    f.account("card", "card", 300);
    assert_eq!(
        f.balance_as_of("bank", None) - f.balance_as_of("card", None),
        700
    );
    assert_eq!(f.monthly("2026-11").await["net_worth"], json!("700"));

    let mut payment = change("f-pay", "f-pay", "payment", "bank", 300, "2026-11-20");
    payment.card_account_id = Some("card".into());
    assert!(matches!(f.apply(&payment), Ok(Outcome::Applied(_))));

    assert_eq!(f.balance_as_of("bank", None), 700);
    assert_eq!(f.balance_as_of("card", None), 0);
    assert_eq!(
        f.postings(),
        vec![
            ("f-pay".into(), "bank".into(), -300),
            ("f-pay".into(), "card".into(), -300),
        ]
    );
    assert_eq!(
        f.count("SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND kind IN ('income','expense')"),
        0
    );

    let report = f.monthly("2026-11").await;
    assert_eq!(report["net_worth"], json!("700"));
    assert_eq!(report["income"], json!("0"));
    assert_eq!(report["expense"], json!("0"));
    assert_eq!(report["net"], json!("0"));
    assert_eq!(report["cash_flow"], json!("-300"));
    f.integrity();
}
