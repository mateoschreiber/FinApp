//! F9.3A: saldo_actual discrepancies require explicit, preview-bound acceptance.
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use finapp::{app, db::Database};
use http_body_util::BodyExt;
use rusqlite::params;
use serde_json::{Value, json};
use tower::ServiceExt;

const ORIGIN: &str = "https://finanzas.example";
const PASSWORD: &str = "contrasena sintetica de saldos";

struct Fixture {
    db: Database,
    router: Router,
    cookie: String,
    user: String,
}

impl Fixture {
    async fn new() -> Self {
        let db = Database::open_memory().unwrap();
        db.bootstrap_admin("alice", PASSWORD).unwrap();
        let router = app(db.clone(), ORIGIN.into());
        let res = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/login")
                    .header("origin", ORIGIN)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"username":"alice","password":PASSWORD}).to_string(),
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
            router,
            cookie,
            user,
        }
    }

    async fn import(&self, mut body: Value) -> (StatusCode, Value) {
        if body.get("preview_token").is_none() {
            body["preview_token"] = Value::Null;
        }
        let res: Response = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/import/xlsx")
                    .header("origin", ORIGIN)
                    .header("content-type", "application/json")
                    .header(header::COOKIE, &self.cookie)
                    .header("x-finapp-user", &self.user)
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }

    fn balance(&self, account: &str) -> i64 {
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
                "SELECT COALESCE(SUM(delta),0) FROM finance_balance_postings WHERE user_id=?1 AND account_id=?2",
                params![self.user, account],
                |r| r.get(0),
            )
            .unwrap();
        initial + delta
    }

    fn count_operations(&self) -> i64 {
        self.db
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM finance_operations WHERE user_id=?1",
                [&self.user],
                |r| r.get(0),
            )
            .unwrap()
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

fn cell(value: &str) -> String {
    format!(r#"<c t="inlineStr"><is><t>{value}</t></is></c>"#)
}

fn sheet(rows: &[Vec<String>]) -> String {
    let mut xml = String::from(
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
    );
    for row in rows {
        xml.push_str("<row>");
        for value in row {
            xml.push_str(&cell(value));
        }
        xml.push_str("</row>");
    }
    xml.push_str("</sheetData></worksheet>");
    xml
}

type AccountRow = (
    &'static str,
    &'static str,
    &'static str,
    i64,
    Option<i64>,
    Option<&'static str>,
);
type MovementRow = (&'static str, &'static str, &'static str, &'static str, i64);

/// Builds a synthetic workbook. `cutoff` and `saldo_actual` are optional per account.
fn xlsx(accounts: &[AccountRow], movements: &[MovementRow]) -> String {
    let cuentas_header: Vec<String> = [
        "cuenta_id",
        "tipo",
        "nombre",
        "saldo_inicial",
        "saldo_actual",
        "fecha_corte",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let mut cuentas = vec![cuentas_header];
    for (id, kind, name, initial, actual, cutoff) in accounts {
        cuentas.push(vec![
            (*id).to_string(),
            (*kind).to_string(),
            (*name).to_string(),
            initial.to_string(),
            actual.map(|v| v.to_string()).unwrap_or_default(),
            cutoff.unwrap_or("").to_string(),
        ]);
    }
    let categorias = vec![
        vec!["categoria_id".into(), "tipo".into(), "nombre".into()],
        vec!["com".into(), "gasto".into(), "Comida".into()],
    ];
    let movimientos_header: Vec<String> = [
        "movimiento_id",
        "tipo",
        "fecha",
        "cuenta_origen_id",
        "categoria_id",
        "monto",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let mut filas = vec![movimientos_header];
    for (id, kind, date, account, amount) in movements {
        filas.push(vec![
            (*id).to_string(),
            (*kind).to_string(),
            (*date).to_string(),
            (*account).to_string(),
            "com".into(),
            amount.to_string(),
        ]);
    }
    let content_types = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/></Types>"#;
    let book = r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Cuentas" sheetId="1" r:id="rId1"/><sheet name="Categorias" sheetId="2" r:id="rId2"/><sheet name="Movimientos" sheetId="3" r:id="rId3"/></sheets></workbook>"#;
    let rel = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet3.xml"/></Relationships>"#;
    let shared = r#"<sst count="1" uniqueCount="1"><si><t>x</t></si></sst>"#;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, content) in [
        ("[Content_Types].xml", content_types),
        ("xl/workbook.xml", book),
        ("xl/_rels/workbook.xml.rels", rel),
        ("xl/sharedStrings.xml", shared),
        ("xl/worksheets/sheet1.xml", &sheet(&cuentas)),
        ("xl/worksheets/sheet2.xml", &sheet(&categorias)),
        ("xl/worksheets/sheet3.xml", &sheet(&filas)),
    ] {
        zip.start_file(name, options).unwrap();
        std::io::Write::write_all(&mut zip, content.as_bytes()).unwrap();
    }
    STANDARD.encode(zip.finish().unwrap().into_inner())
}

fn body(file: &str, token: Option<&str>, ack: Option<&str>) -> Value {
    let mut value = json!({"file":file, "mapping":{}, "include_before_cutoff":true});
    if let Some(token) = token {
        value["preview_token"] = json!(token);
    }
    if let Some(ack) = ack {
        value["ack_saldo"] = json!(ack);
    }
    value
}

#[tokio::test]
async fn a_matching_balance_imports_without_ack() {
    let f = Fixture::new().await;
    let file = xlsx(
        &[("banco", "banco", "Banco", 1000, Some(900), None)],
        &[("g1", "gasto", "2026-03-02", "banco", 100)],
    );
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["differences"], json!([]));
    assert_eq!(report["warnings"], json!([]));
    assert_eq!(report["balances"]["banco"], json!("900"));
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(f.balance("banco"), 900);
    assert_eq!(f.count_operations(), 1);
    f.integrity();
}

#[tokio::test]
async fn b_higher_declared_balance_requires_matching_ack() {
    let f = Fixture::new().await;
    let file = xlsx(
        &[("banco", "banco", "Banco", 1000, Some(1000), None)],
        &[("g1", "gasto", "2026-03-02", "banco", 100)],
    );
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(
        report["differences"],
        json!([{"account_id":"banco","declared_balance":1000,"calculated_balance":900,"difference":100}])
    );
    assert!(
        report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("saldo esperado"))
    );
    let token = report["token"].as_str().unwrap().to_owned();

    let (status, rejected) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    assert_eq!(f.count_operations(), 0, "no partial writes after rejection");
    let (status, wrong) = f
        .import(body(&file, Some(&token), Some("otro-token")))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{wrong}");
    assert_eq!(f.count_operations(), 0);

    let (status, done) = f.import(body(&file, Some(&token), Some(&token))).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(f.balance("banco"), 900);
    f.integrity();
}

#[tokio::test]
async fn c_lower_declared_balance_requires_ack_and_keeps_the_sign() {
    let f = Fixture::new().await;
    let file = xlsx(
        &[("banco", "banco", "Banco", 1000, Some(800), None)],
        &[("g1", "gasto", "2026-03-02", "banco", 100)],
    );
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(
        report["differences"],
        json!([{"account_id":"banco","declared_balance":800,"calculated_balance":900,"difference":-100}])
    );
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, _rejected) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, done) = f.import(body(&file, Some(&token), Some(&token))).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(
        f.balance("banco"),
        900,
        "acceptance never adjusts the balance"
    );
    f.integrity();
}

#[tokio::test]
async fn d_positive_card_debt_declared_as_positive() {
    let f = Fixture::new().await;
    let file = xlsx(&[("tarjeta", "tarjeta", "Visa", 300, Some(300), None)], &[]);
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["differences"], json!([]));
    assert_eq!(report["balances"]["tarjeta"], json!("300"));
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["committed"], json!(true));
    assert_eq!(f.balance("tarjeta"), 300);
    f.integrity();
}

#[tokio::test]
async fn e_negative_card_credit_declared_as_negative() {
    let f = Fixture::new().await;
    let file = xlsx(
        &[("tarjeta", "tarjeta", "Visa", -300, Some(-300), None)],
        &[],
    );
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["differences"], json!([]));
    assert_eq!(report["balances"]["tarjeta"], json!("-300"));
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["committed"], json!(true));
    assert_eq!(f.balance("tarjeta"), -300);
    f.integrity();
}

#[tokio::test]
async fn f_cutoff_excludes_history_from_the_balance() {
    let f = Fixture::new().await;
    let file = xlsx(
        &[(
            "banco",
            "banco",
            "Banco",
            1000,
            Some(1000),
            Some("2026-03-01"),
        )],
        &[("g1", "gasto", "2026-02-15", "banco", 100)],
    );
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["differences"], json!([]));
    assert!(
        report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("anterior a la fecha de corte"))
    );
    assert_eq!(report["balances"]["banco"], json!("1000"));
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["committed"], json!(true));
    assert_eq!(f.balance("banco"), 1000);
    f.integrity();
}

#[tokio::test]
async fn g_existing_account_with_previous_operations_can_be_linked() {
    let f = Fixture::new().await;
    finapp::finance::save_account(
        &f.db,
        &f.user,
        &finapp::finance::Account {
            id: "banco".into(),
            name: "Banco".into(),
            kind: "bank".into(),
            initial_balance: 1000,
        },
    )
    .unwrap();
    finapp::finance::save_category(
        &f.db,
        &f.user,
        &finapp::finance::Category {
            id: "previa".into(),
            name: "Previa".into(),
            kind: "expense".into(),
        },
    )
    .unwrap();
    let change = finapp::finance::Change {
        request_id: "previo".into(),
        id: "previo".into(),
        expected_version: 0,
        deleted: false,
        kind: Some("expense".into()),
        account_id: Some("banco".into()),
        category_id: Some("previa".into()),
        card_account_id: None,
        destination_account_id: None,
        original_operation_id: None,
        amount: Some(100),
        date: Some("2026-01-10".into()),
        description: None,
        notes: None,
    };
    finapp::finance::apply(&f.db, &f.user, &change).unwrap();
    let file = xlsx(&[("banco", "banco", "Banco", 1000, Some(800), None)], &[]);
    let mut value = body(&file, None, None);
    value["mapping"] = json!({"banco":"banco"});
    let (status, report) = f.import(value).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(
        report["differences"],
        json!([{"account_id":"banco","declared_balance":800,"calculated_balance":900,"difference":-100}])
    );
    let token = report["token"].as_str().unwrap().to_owned();
    let mut reject = body(&file, Some(&token), None);
    reject["mapping"] = json!({"banco":"banco"});
    let (status, _) = f.import(reject).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let mut accept = body(&file, Some(&token), Some(&token));
    accept["mapping"] = json!({"banco":"banco"});
    let (status, done) = f.import(accept).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(f.balance("banco"), 900);
    f.integrity();
}

#[tokio::test]
async fn h_multiple_accounts_report_only_the_discrepant_one() {
    let f = Fixture::new().await;
    let file = xlsx(
        &[
            ("uno", "banco", "Uno", 1000, Some(900), None),
            ("dos", "banco", "Dos", 0, Some(500), None),
        ],
        &[("g1", "gasto", "2026-03-02", "uno", 100)],
    );
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(
        report["differences"],
        json!([{"account_id":"dos","declared_balance":500,"calculated_balance":0,"difference":500}])
    );
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&file, Some(&token), Some(&token))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["committed"], json!(true));
    assert_eq!(f.balance("uno"), 900);
    assert_eq!(f.balance("dos"), 0);
    f.integrity();
}

#[tokio::test]
async fn i_missing_saldo_actual_never_invents_a_difference() {
    let f = Fixture::new().await;
    let file = xlsx(&[("banco", "banco", "Banco", 1000, None, None)], &[]);
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["differences"], json!([]));
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["committed"], json!(true));
    f.integrity();
}

#[tokio::test]
async fn j_partial_history_can_be_accepted_once_reviewed() {
    let f = Fixture::new().await;
    let file = xlsx(
        &[("banco", "banco", "Banco", 1000, Some(500), None)],
        &[("g1", "gasto", "2026-03-02", "banco", 100)],
    );
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(
        report["differences"],
        json!([{"account_id":"banco","declared_balance":500,"calculated_balance":900,"difference":-400}])
    );
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, rejected) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    let (status, done) = f.import(body(&file, Some(&token), Some(&token))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["committed"], json!(true));
    assert_eq!(f.balance("banco"), 900);
    assert_eq!(
        f.count_operations(),
        1,
        "no automatic adjustment operation is created"
    );
    f.integrity();
}

#[tokio::test]
async fn stale_ack_from_another_preview_is_rejected() {
    let f = Fixture::new().await;
    let first = xlsx(
        &[("banco", "banco", "Banco", 1000, Some(1000), None)],
        &[("g1", "gasto", "2026-03-02", "banco", 100)],
    );
    let second = xlsx(
        &[("banco", "banco", "Banco", 1000, Some(800), None)],
        &[("g1", "gasto", "2026-03-02", "banco", 100)],
    );
    let (_, preview_one) = f.import(body(&first, None, None)).await;
    let token_one = preview_one["token"].as_str().unwrap().to_owned();
    let (_, preview_two) = f.import(body(&second, None, None)).await;
    let token_two = preview_two["token"].as_str().unwrap().to_owned();
    assert_ne!(token_one, token_two);

    let (status, _) = f
        .import(body(&second, Some(&token_two), Some(&token_one)))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(f.count_operations(), 0);
    let (status, done) = f
        .import(body(&second, Some(&token_two), Some(&token_two)))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["committed"], json!(true));
    f.integrity();
}

#[tokio::test]
async fn changed_file_or_mapping_invalidates_the_ack() {
    let f = Fixture::new().await;
    let first = xlsx(
        &[("banco", "banco", "Banco", 1000, Some(1000), None)],
        &[("g1", "gasto", "2026-03-02", "banco", 100)],
    );
    let second = xlsx(&[("banco", "banco", "Banco", 1000, Some(900), None)], &[]);
    let (_, preview) = f.import(body(&first, None, None)).await;
    let token = preview["token"].as_str().unwrap().to_owned();

    let (status, _) = f.import(body(&second, Some(&token), Some(&token))).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a token cannot authorize another file"
    );

    let mut remapped = body(&first, None, None);
    remapped["mapping"] = json!({"banco":"banco"});
    let (_, other) = f.import(remapped).await;
    let other_token = other["token"].as_str().unwrap().to_owned();
    assert_ne!(token, other_token);
    let mut attempt = body(&first, Some(&other_token), Some(&token));
    attempt["mapping"] = json!({"banco":"banco"});
    let (status, _) = f.import(attempt).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "an ack from another mapping is rejected"
    );
    assert_eq!(f.count_operations(), 0);
    f.integrity();
}

#[tokio::test]
async fn errors_block_confirmation_even_with_ack() {
    let f = Fixture::new().await;
    let file = xlsx(
        &[("banco", "banco", "Banco", 1000, Some(1000), None)],
        &[
            ("g1", "gasto", "2026-03-02", "banco", 100),
            ("g2", "gasto", "2026-03-02", "banco", -5),
        ],
    );
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert!(!report["errors"].as_array().unwrap().is_empty());
    assert_eq!(
        report["differences"],
        json!([{"account_id":"banco","declared_balance":1000,"calculated_balance":900,"difference":100}])
    );
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, rejected) = f.import(body(&file, Some(&token), Some(&token))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    assert_eq!(
        f.count_operations(),
        0,
        "errors block the whole confirmation"
    );
    f.integrity();
}

#[tokio::test]
async fn repeated_confirmation_after_commit_is_rejected() {
    let f = Fixture::new().await;
    let file = xlsx(
        &[("banco", "banco", "Banco", 1000, Some(900), None)],
        &[("g1", "gasto", "2026-03-02", "banco", 100)],
    );
    let (_, preview) = f.import(body(&file, None, None)).await;
    let token = preview["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["committed"], json!(true));
    let (status, again) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{again}");
    assert_eq!(f.count_operations(), 1, "no duplicated movements");
    f.integrity();
}

#[tokio::test]
async fn another_user_cannot_confirm_with_a_foreign_token() {
    let f = Fixture::new().await;
    let file = xlsx(
        &[("banco", "banco", "Banco", 1000, Some(1000), None)],
        &[("g1", "gasto", "2026-03-02", "banco", 100)],
    );
    let (_, preview) = f.import(body(&file, None, None)).await;
    let token = preview["token"].as_str().unwrap().to_owned();

    let create = f
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/users")
                .header("origin", ORIGIN)
                .header("content-type", "application/json")
                .header(header::COOKIE, &f.cookie)
                .header("x-finapp-user", &f.user)
                .body(Body::from(
                    json!({"username":"bob","password":PASSWORD}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let login = f
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/login")
                .header("origin", ORIGIN)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"username":"bob","password":PASSWORD}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let bob_cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let bob: Value =
        serde_json::from_slice(&login.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let bob_id = bob["id"].as_str().unwrap().to_owned();

    let res = f
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/import/xlsx")
                .header("origin", ORIGIN)
                .header("content-type", "application/json")
                .header(header::COOKIE, &bob_cookie)
                .header("x-finapp-user", &bob_id)
                .body(Body::from(
                    body(&file, Some(&token), Some(&token)).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let text =
        String::from_utf8_lossy(&res.into_body().collect().await.unwrap().to_bytes()).to_string();
    assert!(text.contains("La vista previa cambió"), "{text}");
    f.integrity();
}
