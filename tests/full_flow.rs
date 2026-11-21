use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use finapp::{app, db::Database};
use http_body_util::BodyExt;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    io::{Cursor, Write},
    path::PathBuf,
};
use tower::ServiceExt;

const ORIGIN: &str = "https://finanzas.example";
const PASSWORD: &str = "contraseña temporal sintética";
// calamine 0.38 resolves the workbook through the package-level relationships.
const PACKAGE_RELS: &str = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;

async fn request(
    app: &Router,
    session: Option<&Session>,
    method: &str,
    path: &str,
    value: Value,
) -> Response {
    let mut req = Request::builder().method(method).uri(path);
    if method != "GET" {
        req = req
            .header("origin", ORIGIN)
            .header("content-type", "application/json");
    }
    if let Some(session) = session {
        req = req
            .header("cookie", &session.cookie)
            .header("x-finapp-user", &session.id);
    }
    app.clone()
        .oneshot(req.body(Body::from(value.to_string())).unwrap())
        .await
        .unwrap()
}

async fn checked_json(res: Response, status: StatusCode) -> Value {
    let actual = res.status();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(actual, status, "{}", String::from_utf8_lossy(&body));
    if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    }
}

struct Session {
    id: String,
    cookie: String,
}
async fn login(app: &Router, name: &str) -> Session {
    let res = request(
        app,
        None,
        "POST",
        "/api/login",
        json!({"username":name,"password":PASSWORD}),
    )
    .await;
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
    let id = checked_json(res, StatusCode::OK).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    Session { id, cookie }
}

fn workbook() -> String {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let sheets = [
        (
            "Cuentas",
            vec![
                vec![
                    "cuenta_id",
                    "nombre",
                    "tipo",
                    "saldo_inicial",
                    "fecha_corte",
                    "saldo_actual",
                ],
                vec![
                    "banco-historico",
                    "Banco histórico",
                    "Banco",
                    "1000",
                    "2026-03-01",
                    "900",
                ],
            ],
        ),
        (
            "Categorias",
            vec![
                vec!["categoria_id", "nombre", "tipo"],
                vec!["comidas", "Comidas", "Gasto"],
            ],
        ),
        (
            "Movimientos",
            vec![
                vec![
                    "movimiento_id",
                    "tipo",
                    "fecha",
                    "cuenta_origen_id",
                    "categoria_id",
                    "monto",
                    "descripcion",
                ],
                vec![
                    "anterior",
                    "gasto",
                    "28/2/2026",
                    "banco-historico",
                    "comidas",
                    "200",
                    "Antes del corte",
                ],
                vec![
                    "actual",
                    "gasto",
                    "2/3/2026",
                    "banco-historico",
                    "comidas",
                    "100",
                    "Después del corte",
                ],
            ],
        ),
    ];
    let content_types = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/></Types>"#;
    let mut book=r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>"#.to_owned();
    let mut rel =
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#
            .to_owned();
    for (index, (name, rows)) in sheets.iter().enumerate() {
        let n = index + 1;
        book.push_str(&format!(
            r#"<sheet name="{name}" sheetId="{n}" r:id="rId{n}"/>"#
        ));
        rel.push_str(&format!(r#"<Relationship Id="rId{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{n}.xml"/>"#));
        let mut sheet=r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#.to_owned();
        for (line, row) in rows.iter().enumerate() {
            let line = line + 1;
            sheet.push_str(&format!(r#"<row r="{line}">"#));
            for (column, value) in row.iter().enumerate() {
                let column = char::from(b'A' + column as u8);
                sheet.push_str(&format!(
                    r#"<c r="{column}{line}" t="inlineStr"><is><t>{value}</t></is></c>"#
                ));
            }
            sheet.push_str("</row>");
        }
        sheet.push_str("</sheetData></worksheet>");
        zip.start_file(format!("xl/worksheets/sheet{n}.xml"), options)
            .unwrap();
        zip.write_all(sheet.as_bytes()).unwrap();
    }
    book.push_str("</sheets></workbook>");
    rel.push_str("</Relationships>");
    for (name, text) in [
        ("_rels/.rels", PACKAGE_RELS),
        ("[Content_Types].xml", content_types),
        ("xl/workbook.xml", &book),
        ("xl/_rels/workbook.xml.rels", &rel),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    STANDARD.encode(zip.finish().unwrap().into_inner())
}

#[tokio::test]
async fn xlsx_preview_confirmation_cutoff_and_month_review() {
    let db = Database::open_memory().unwrap();
    db.bootstrap_admin("alice", PASSWORD).unwrap();
    let router = app(db, ORIGIN.into());
    let alice = login(&router, "alice").await;
    let bob_record = request(
        &router,
        Some(&alice),
        "POST",
        "/api/users",
        json!({"username":"bob","password":PASSWORD}),
    )
    .await;
    checked_json(bob_record, StatusCode::CREATED).await;
    let bob = login(&router, "bob").await;

    let file = workbook();
    let preview_path = "/api/import/xlsx";
    let post = |path: &'static str, data| async {
        checked_json(
            request(&router, Some(&alice), "POST", path, data).await,
            StatusCode::OK,
        )
        .await
    };
    let missing = post(preview_path, json!({"file":file})).await;
    assert_eq!(missing["errors"].as_array().unwrap().len(), 1);
    let payload = json!({"file":file,"include_before_cutoff":true});
    let report = post(preview_path, payload.clone()).await;
    assert_eq!(report["added"], 4);
    assert_eq!(report["balances"]["banco-historico"], "900");
    assert_eq!(report["errors"], json!([]));
    let before = checked_json(
        request(&router, Some(&alice), "GET", "/api/snapshot", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(before["operations"], json!([]));
    let mut commit = payload.clone();
    commit["preview_token"] = report["token"].clone();
    assert_eq!(post(preview_path, commit).await["committed"], true);
    assert_eq!(post(preview_path, payload).await["duplicates"], 4);

    let monthly = "/api/planning/month/2026-03";
    let view = checked_json(
        request(&router, Some(&alice), "GET", monthly, Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(view["expense"], "100");
    assert_eq!(view["net_worth"], "900");
    checked_json(
        request(
            &router,
            Some(&alice),
            "POST",
            monthly,
            json!({"digest":view["digest"]}),
        )
        .await,
        StatusCode::NO_CONTENT,
    )
    .await;
    let reviewed = checked_json(
        request(&router, Some(&alice), "GET", monthly, Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(reviewed["reviewed"], true);

    let receipt=post("/api/receipts",json!({"operation_id":"actual","name":"ticket.png","mime":"image/png","content":"89504e470d0a1a0a00000000"})).await;
    let receipt_path = format!("/api/receipts/{}", receipt["id"].as_str().unwrap());
    let bob_read = request(&router, Some(&bob), "GET", &receipt_path, Value::Null).await;
    assert_eq!(bob_read.status(), StatusCode::NOT_FOUND);
    let alice_read = request(&router, Some(&alice), "GET", &receipt_path, Value::Null).await;
    assert_eq!(alice_read.status(), StatusCode::OK);
    assert_eq!(
        alice_read.headers()[header::CONTENT_DISPOSITION],
        "attachment"
    );

    post("/api/operations",json!({
        "request_id":"edit","id":"actual","expected_version":1,"deleted":false,
        "kind":"expense","account_id":"banco-historico","category_id":"comidas","date":"2026-03-02","amount":150
    })).await;
    let renewed = checked_json(
        request(&router, Some(&alice), "GET", monthly, Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(renewed["reviewed"], false);
    assert_eq!(renewed["expense"], "150");
    assert_eq!(renewed["net_worth"], "850");
    assert_eq!(renewed["changes"][0]["previous"]["amount"], 100);
    assert_eq!(renewed["changes"][0]["current"]["amount"], 150);
    let csv = request(
        &router,
        Some(&alice),
        "GET",
        "/api/export/informe?month=2026-03",
        Value::Null,
    )
    .await;
    assert_eq!(csv.status(), StatusCode::OK);
    assert_eq!(
        csv.headers()[header::CONTENT_TYPE],
        "text/csv; charset=utf-8"
    );
    let body = csv.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&body).contains("Patrimonio,,850"));
}

struct TemporaryDatabase(PathBuf);
impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn opening_legacy_sqlite_preserves_versions_deletions_and_idempotency() {
    let path = TemporaryDatabase(
        std::env::temp_dir().join(format!("finapp-migration-{}.sqlite", uuid::Uuid::new_v4())),
    );
    let conn = Connection::open(&path.0).unwrap();
    conn.execute_batch(include_str!("../migrations/001_auth.sql"))
        .unwrap();
    conn.execute_batch(include_str!("../migrations/002_finance.sql"))
        .unwrap();
    conn.execute_batch(
        "INSERT INTO finance_accounts VALUES('u','bank','Banco','bank',100);
        INSERT INTO finance_categories VALUES('u','food','Comida','expense');",
    )
    .unwrap();
    let old_change = r#"{"request_id":"same-request","id":"kept","expected_version":1,"deleted":false,"kind":"expense","account_id":"bank","category_id":"food","card_account_id":null,"amount":25,"date":"2026-02-03","description":null,"notes":null}"#;
    let operation = r#"{"id":"kept","version":2,"deleted":false,"kind":"expense","account_id":"bank","category_id":"food","card_account_id":null,"amount":25,"date":"2026-02-03","description":null,"notes":null}"#;
    conn.execute(
        "INSERT INTO finance_operations VALUES('u','kept',2,?)",
        [operation],
    )
    .unwrap();
    let removed = operation
        .replace("kept", "removed")
        .replace("false", "true");
    conn.execute(
        "INSERT INTO finance_operations VALUES('u','removed',2,?)",
        [removed],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO finance_requests VALUES('u','same-request',?,?)",
        params![
            old_change,
            format!(r#"{{"status":"applied","operation":{operation}}}"#)
        ],
    )
    .unwrap();
    drop(conn);

    for _ in 0..2 {
        let db = Database::open(&path.0).unwrap();
        let snap = finapp::finance::snapshot(&db, "u").unwrap();
        assert_eq!(snap.operations.len(), 2);
        assert!(
            snap.operations
                .iter()
                .any(|o| o.id == "removed" && o.deleted)
        );
        let replay =
            finapp::finance::apply(&db, "u", &serde_json::from_str(old_change).unwrap()).unwrap();
        assert!(
            matches!(replay,finapp::finance::Outcome::Applied(o) if o.version==2 && o.amount==25)
        );
        let conn = db.0.lock().unwrap();
        let delta:i64=conn.query_row("SELECT SUM(delta) FROM finance_balance_postings WHERE user_id='u' AND account_id='bank'",[],|r|r.get(0)).unwrap();
        assert_eq!(delta, -25);
        let foreign_keys: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(foreign_keys, 0);
    }
}

#[tokio::test]
async fn financial_changes_return_reviewed_later_months_to_pending() {
    let db = Database::open_memory().unwrap();
    db.bootstrap_admin("alice", PASSWORD).unwrap();
    let router = app(db, ORIGIN.into());
    let alice = login(&router, "alice").await;
    let post = |path: &'static str, data: Value| async {
        checked_json(
            request(&router, Some(&alice), "POST", path, data).await,
            StatusCode::OK,
        )
        .await
    };
    post(
        "/api/accounts",
        json!({"id":"bank","name":"Banco","kind":"bank","initial_balance":0}),
    )
    .await;
    post(
        "/api/categories",
        json!({"id":"food","name":"Comidas","kind":"expense"}),
    )
    .await;
    post("/api/operations", json!({
        "request_id":"oct","id":"gasto-oct","expected_version":0,"deleted":false,
        "kind":"expense","account_id":"bank","category_id":"food","date":"2026-10-10","amount":200
    })).await;
    let monthly = "/api/planning/month/2026-11";
    let november = checked_json(
        request(&router, Some(&alice), "GET", monthly, Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(november["net_worth"], "-200");
    checked_json(
        request(
            &router,
            Some(&alice),
            "POST",
            monthly,
            json!({"digest":november["digest"]}),
        )
        .await,
        StatusCode::NO_CONTENT,
    )
    .await;
    let reviewed = checked_json(
        request(&router, Some(&alice), "GET", monthly, Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(reviewed["reviewed"], true);

    // An October edit changes November's balance without touching November's own rows.
    post("/api/operations", json!({
        "request_id":"oct-edit","id":"gasto-oct","expected_version":1,"deleted":false,
        "kind":"expense","account_id":"bank","category_id":"food","date":"2026-10-10","amount":300
    })).await;
    let after_edit = checked_json(
        request(&router, Some(&alice), "GET", monthly, Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(after_edit["net_worth"], "-300");
    assert_eq!(after_edit["reviewed"], false);
    checked_json(
        request(
            &router,
            Some(&alice),
            "POST",
            monthly,
            json!({"digest":after_edit["digest"]}),
        )
        .await,
        StatusCode::NO_CONTENT,
    )
    .await;

    // Editing an opening balance has no trigger; the digest must still notice.
    post(
        "/api/accounts",
        json!({"id":"bank","name":"Banco","kind":"bank","initial_balance":1000}),
    )
    .await;
    let after_balance = checked_json(
        request(&router, Some(&alice), "GET", monthly, Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(after_balance["net_worth"], "700");
    assert_eq!(after_balance["reviewed"], false);
    checked_json(
        request(
            &router,
            Some(&alice),
            "POST",
            monthly,
            json!({"digest":after_balance["digest"]}),
        )
        .await,
        StatusCode::NO_CONTENT,
    )
    .await;

    // So does moving the account cutoff past the reviewed month.
    post("/api/accounts", json!({"id":"bank","name":"Banco","kind":"bank","initial_balance":1000,
        "details":{"entity":"account","archived":false,"institution":"","notes":"","cutoff_date":"2026-12-01"}})).await;
    let after_cutoff = checked_json(
        request(&router, Some(&alice), "GET", monthly, Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(after_cutoff["net_worth"], "0");
    assert_eq!(after_cutoff["reviewed"], false);
    checked_json(
        request(
            &router,
            Some(&alice),
            "POST",
            monthly,
            json!({"digest":after_cutoff["digest"]}),
        )
        .await,
        StatusCode::NO_CONTENT,
    )
    .await;

    // A change in a later month leaves the reviewed month alone.
    post("/api/operations", json!({
        "request_id":"dic","id":"gasto-dic","expected_version":0,"deleted":false,
        "kind":"expense","account_id":"bank","category_id":"food","date":"2026-12-05","amount":50
    })).await;
    let after_december = checked_json(
        request(&router, Some(&alice), "GET", monthly, Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(after_december["reviewed"], true);
}

/// F12.0: deleting something that no longer exists answers 404, not a silent 204.
#[tokio::test]
async fn missing_deletes_answer_404() {
    let db = Database::open_memory().unwrap();
    db.bootstrap_admin("alice", PASSWORD).unwrap();
    let router = app(db.clone(), ORIGIN.into());
    let alice = login(&router, "alice").await;
    for plan in [
        json!({"kind":"remove_budget","month":"2026-11","category_id":"ghost"}),
        json!({"kind":"remove_recurring","id":"ghost"}),
        json!({"kind":"remove_goal","id":"ghost"}),
    ] {
        checked_json(
            request(&router, Some(&alice), "POST", "/api/planning", plan).await,
            StatusCode::NOT_FOUND,
        )
        .await;
    }
    checked_json(
        request(
            &router,
            Some(&alice),
            "DELETE",
            "/api/receipts/ghost",
            Value::Null,
        )
        .await,
        StatusCode::NOT_FOUND,
    )
    .await;

    // The happy path still works and the second attempt reports 404.
    checked_json(
        request(
            &router,
            Some(&alice),
            "POST",
            "/api/accounts",
            json!({"id":"cash","name":"Efectivo","kind":"cash","initial_balance":0}),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    checked_json(
        request(
            &router,
            Some(&alice),
            "POST",
            "/api/planning",
            json!({"kind":"goal","id":"g1","name":"Meta","account_id":"cash","target":100,"deadline":"2026-12"}),
        )
        .await,
        StatusCode::NO_CONTENT,
    )
    .await;
    checked_json(
        request(
            &router,
            Some(&alice),
            "POST",
            "/api/planning",
            json!({"kind":"remove_goal","id":"g1"}),
        )
        .await,
        StatusCode::NO_CONTENT,
    )
    .await;
    checked_json(
        request(
            &router,
            Some(&alice),
            "POST",
            "/api/planning",
            json!({"kind":"remove_goal","id":"g1"}),
        )
        .await,
        StatusCode::NOT_FOUND,
    )
    .await;
}
