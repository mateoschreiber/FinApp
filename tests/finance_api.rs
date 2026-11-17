use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use finapp::{
    db::Database,
    finance::{self, Account, Category, Change, Outcome},
};
use tower::ServiceExt;

#[tokio::test]
async fn browser_assets_are_served_with_javascript_mime_type() {
    let app = finapp::app(
        Database::open_memory().unwrap(),
        "https://finanzas.example".to_owned(),
    );
    let response = app
        .oneshot(
            Request::builder()
                .uri("/offline-store.mjs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("javascript")
    );
}

#[tokio::test]
async fn financial_requests_are_bound_to_authenticated_user() {
    use http_body_util::BodyExt;
    let db = Database::open_memory().unwrap();
    db.bootstrap_admin("admin", "password-1234").unwrap();
    let app = finapp::app(db, "https://finanzas.example".to_owned());
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/login")
                .header("content-type", "application/json")
                .header("origin", "https://finanzas.example")
                .body(Body::from(
                    r#"{"username":"admin","password":"password-1234"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let cookie = login
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let login_body: serde_json::Value =
        serde_json::from_slice(&login.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let user_id = login_body["id"].as_str().unwrap();
    for (supplied, status) in [
        (None, StatusCode::FORBIDDEN),
        (Some("another-user"), StatusCode::FORBIDDEN),
        (Some(user_id), StatusCode::OK),
    ] {
        let mut request = Request::builder()
            .uri("/api/snapshot")
            .header("cookie", &cookie);
        if let Some(id) = supplied {
            request = request.header("x-finapp-user", id);
        }
        assert_eq!(
            app.clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status(),
            status
        );
    }
    for (path, body) in [
        (
            "/api/accounts",
            r#"{"id":"a","name":"Banco","kind":"bank","initial_balance":0}"#,
        ),
        (
            "/api/categories",
            r#"{"id":"c","name":"Comida","kind":"expense"}"#,
        ),
        (
            "/api/operations",
            r#"{"request_id":"r","id":"m","expected_version":0,"deleted":true}"#,
        ),
    ] {
        for supplied in [None, Some("another-user")] {
            let mut request = Request::builder()
                .method("POST")
                .uri(path)
                .header("cookie", &cookie)
                .header("origin", "https://finanzas.example")
                .header("content-type", "application/json");
            if let Some(id) = supplied {
                request = request.header("x-finapp-user", id);
            }
            assert_eq!(
                app.clone()
                    .oneshot(request.body(Body::from(body)).unwrap())
                    .await
                    .unwrap()
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
    }
}

#[test]
fn resulting_balances_remain_exact_in_javascript() {
    let db = setup();
    finance::save_account(
        &db,
        "alice",
        &Account {
            id: "bank".into(),
            name: "Banco".into(),
            kind: "bank".into(),
            initial_balance: 9_007_199_254_740_991,
        },
    )
    .unwrap();
    finance::save_category(
        &db,
        "alice",
        &Category {
            id: "salary".into(),
            name: "Sueldo".into(),
            kind: "income".into(),
        },
    )
    .unwrap();
    let c = Change {
        request_id: "r".into(),
        id: "m".into(),
        expected_version: 0,
        deleted: false,
        kind: Some("income".into()),
        account_id: Some("bank".into()),
        category_id: Some("salary".into()),
        card_account_id: None,
        amount: Some(1),
        date: Some("2026-11-14".into()),
        description: None,
        notes: None,
    };
    assert!(finance::apply(&db, "alice", &c).is_err());
    assert!(
        finance::snapshot(&db, "alice")
            .unwrap()
            .operations
            .is_empty()
    );
}

#[test]
fn opening_balance_edits_and_card_payments_cannot_overflow_or_partially_apply() {
    let db = setup();
    for (id, kind, initial_balance) in [
        ("bank", "bank", 100),
        ("card", "card", -9_007_199_254_740_991),
    ] {
        finance::save_account(
            &db,
            "alice",
            &Account {
                id: id.into(),
                name: id.into(),
                kind: kind.into(),
                initial_balance,
            },
        )
        .unwrap();
    }
    let payment = Change {
        request_id: "payment".into(),
        id: "p".into(),
        expected_version: 0,
        deleted: false,
        kind: Some("payment".into()),
        account_id: Some("bank".into()),
        category_id: None,
        card_account_id: Some("card".into()),
        amount: Some(1),
        date: Some("2026-11-14".into()),
        description: None,
        notes: None,
    };
    assert!(finance::apply(&db, "alice", &payment).is_err());
    assert!(
        finance::snapshot(&db, "alice")
            .unwrap()
            .operations
            .is_empty()
    );
    finance::save_category(
        &db,
        "alice",
        &Category {
            id: "salary".into(),
            name: "Sueldo".into(),
            kind: "income".into(),
        },
    )
    .unwrap();
    let income = Change {
        request_id: "income".into(),
        id: "i".into(),
        expected_version: 0,
        deleted: false,
        kind: Some("income".into()),
        account_id: Some("bank".into()),
        category_id: Some("salary".into()),
        card_account_id: None,
        amount: Some(1),
        date: Some("2026-11-14".into()),
        description: None,
        notes: None,
    };
    finance::apply(&db, "alice", &income).unwrap();
    assert!(
        finance::save_account(
            &db,
            "alice",
            &Account {
                id: "bank".into(),
                name: "Banco".into(),
                kind: "bank".into(),
                initial_balance: 9_007_199_254_740_991
            }
        )
        .is_err()
    );
    let bank = finance::snapshot(&db, "alice")
        .unwrap()
        .accounts
        .into_iter()
        .find(|account| account.id == "bank")
        .unwrap();
    assert_eq!(bank.initial_balance, 100);
}

fn setup() -> Database {
    let db = Database::open_memory().unwrap();
    finance::initialize(&db).unwrap();
    db
}

#[test]
fn mutations_are_idempotent_and_conflicts_keep_server_version() {
    let db = setup();
    finance::save_account(
        &db,
        "alice",
        &Account {
            id: "bank".into(),
            name: "Banco".into(),
            kind: "bank".into(),
            initial_balance: 100,
        },
    )
    .unwrap();
    finance::save_category(
        &db,
        "alice",
        &Category {
            id: "food".into(),
            name: "Comida".into(),
            kind: "expense".into(),
        },
    )
    .unwrap();
    let change = Change {
        request_id: "r1".into(),
        id: "m1".into(),
        expected_version: 0,
        deleted: false,
        kind: Some("expense".into()),
        account_id: Some("bank".into()),
        category_id: Some("food".into()),
        card_account_id: None,
        amount: Some(25),
        date: Some("2026-11-14".into()),
        description: None,
        notes: None,
    };
    let first = finance::apply(&db, "alice", &change).unwrap();
    assert_eq!(first, finance::apply(&db, "alice", &change).unwrap());
    let mut edit = change.clone();
    edit.request_id = "r2".into();
    edit.expected_version = 1;
    edit.amount = Some(30);
    assert!(matches!(
        finance::apply(&db, "alice", &edit).unwrap(),
        Outcome::Applied(_)
    ));
    edit.request_id = "r3".into();
    edit.amount = Some(40);
    match finance::apply(&db, "alice", &edit).unwrap() {
        Outcome::Conflict(Some(current)) => {
            assert_eq!(current.version, 2);
            assert_eq!(current.amount, 30);
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(finance::snapshot(&db, "bob").unwrap().operations.is_empty());
}

#[test]
fn card_payment_is_one_operation_and_validates_account_ownership() {
    let db = setup();
    finance::save_account(
        &db,
        "alice",
        &Account {
            id: "bank".into(),
            name: "Banco".into(),
            kind: "bank".into(),
            initial_balance: 100,
        },
    )
    .unwrap();
    finance::save_account(
        &db,
        "bob",
        &Account {
            id: "card".into(),
            name: "Tarjeta".into(),
            kind: "card".into(),
            initial_balance: 20,
        },
    )
    .unwrap();
    let change = Change {
        request_id: "r".into(),
        id: "p".into(),
        expected_version: 0,
        deleted: false,
        kind: Some("payment".into()),
        account_id: Some("bank".into()),
        category_id: None,
        card_account_id: Some("card".into()),
        amount: Some(20),
        date: Some("2026-11-14".into()),
        description: None,
        notes: None,
    };
    assert!(finance::apply(&db, "alice", &change).is_err());
    assert!(
        finance::snapshot(&db, "alice")
            .unwrap()
            .operations
            .is_empty()
    );
}

#[test]
fn payment_edit_delete_and_duplicate_request_preserve_single_record() {
    let db = setup();
    for (id, kind) in [("bank", "bank"), ("card", "card")] {
        finance::save_account(
            &db,
            "alice",
            &Account {
                id: id.into(),
                name: id.into(),
                kind: kind.into(),
                initial_balance: 100,
            },
        )
        .unwrap();
    }
    let mut c = Change {
        request_id: "first".into(),
        id: "payment".into(),
        expected_version: 0,
        deleted: false,
        kind: Some("payment".into()),
        account_id: Some("bank".into()),
        category_id: None,
        card_account_id: Some("card".into()),
        amount: Some(20),
        date: Some("2026-11-14".into()),
        description: None,
        notes: None,
    };
    finance::apply(&db, "alice", &c).unwrap();
    c.amount = Some(30);
    assert!(
        finance::apply(&db, "alice", &c).is_err(),
        "same request id must never represent different content"
    );
    c.request_id = "edit".into();
    c.expected_version = 1;
    finance::apply(&db, "alice", &c).unwrap();
    let snapshot = finance::snapshot(&db, "alice").unwrap();
    assert_eq!(snapshot.operations.len(), 1);
    assert_eq!(snapshot.operations[0].amount, 30);
    c.request_id = "delete".into();
    c.expected_version = 2;
    c.deleted = true;
    finance::apply(&db, "alice", &c).unwrap();
    let snapshot = finance::snapshot(&db, "alice").unwrap();
    assert_eq!(snapshot.operations.len(), 1);
    assert!(snapshot.operations[0].deleted);
    assert_eq!(snapshot.operations[0].version, 3);
}

#[test]
fn invalid_calendar_date_and_mismatched_category_cannot_be_recorded() {
    let db = setup();
    finance::save_account(
        &db,
        "alice",
        &Account {
            id: "bank".into(),
            name: "Banco".into(),
            kind: "bank".into(),
            initial_balance: 0,
        },
    )
    .unwrap();
    finance::save_category(
        &db,
        "alice",
        &Category {
            id: "salary".into(),
            name: "Sueldo".into(),
            kind: "income".into(),
        },
    )
    .unwrap();
    let mut c = Change {
        request_id: "r".into(),
        id: "m".into(),
        expected_version: 0,
        deleted: false,
        kind: Some("expense".into()),
        account_id: Some("bank".into()),
        category_id: Some("salary".into()),
        card_account_id: None,
        amount: Some(1),
        date: Some("2026-02-30".into()),
        description: None,
        notes: None,
    };
    assert!(finance::apply(&db, "alice", &c).is_err());
    c.date = Some("2026-02-28".into());
    assert!(finance::apply(&db, "alice", &c).is_err());
    assert!(
        finance::snapshot(&db, "alice")
            .unwrap()
            .operations
            .is_empty()
    );
}
