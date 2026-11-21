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
// calamine 0.38 resolves the workbook through the package-level relationships.
const PACKAGE_RELS: &str = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;

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
type CategoryRow = (&'static str, &'static str, &'static str);
type RefMovement = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    i64,
    Option<&'static str>,
);

/// Builds a synthetic workbook with explicit categories and optional related-movement ids.
fn xlsx_full(
    accounts: &[AccountRow],
    categories: &[CategoryRow],
    movements: &[RefMovement],
) -> String {
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
    let mut categorias = vec![vec![
        "categoria_id".to_string(),
        "tipo".to_string(),
        "nombre".to_string(),
    ]];
    for (id, kind, name) in categories {
        categorias.push(vec![
            (*id).to_string(),
            (*kind).to_string(),
            (*name).to_string(),
        ]);
    }
    let movimientos_header: Vec<String> = [
        "movimiento_id",
        "tipo",
        "fecha",
        "cuenta_origen_id",
        "categoria_id",
        "monto",
        "movimiento_relacionado_id",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let mut filas = vec![movimientos_header];
    for (id, kind, date, account, category, amount, related) in movements {
        filas.push(vec![
            (*id).to_string(),
            (*kind).to_string(),
            (*date).to_string(),
            (*account).to_string(),
            (*category).to_string(),
            amount.to_string(),
            related.unwrap_or("").to_string(),
        ]);
    }
    package(&cuentas, &categorias, &filas)
}

fn package(cuentas: &[Vec<String>], categorias: &[Vec<String>], filas: &[Vec<String>]) -> String {
    let content_types = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/></Types>"#;
    let book = r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Cuentas" sheetId="1" r:id="rId1"/><sheet name="Categorias" sheetId="2" r:id="rId2"/><sheet name="Movimientos" sheetId="3" r:id="rId3"/></sheets></workbook>"#;
    let rel = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet3.xml"/></Relationships>"#;
    let shared = r#"<sst count="1" uniqueCount="1"><si><t>x</t></si></sst>"#;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, content) in [
        ("_rels/.rels", PACKAGE_RELS),
        ("[Content_Types].xml", content_types),
        ("xl/workbook.xml", book),
        ("xl/_rels/workbook.xml.rels", rel),
        ("xl/sharedStrings.xml", shared),
        ("xl/worksheets/sheet1.xml", &sheet(cuentas)),
        ("xl/worksheets/sheet2.xml", &sheet(categorias)),
        ("xl/worksheets/sheet3.xml", &sheet(filas)),
    ] {
        zip.start_file(name, options).unwrap();
        std::io::Write::write_all(&mut zip, content.as_bytes()).unwrap();
    }
    STANDARD.encode(zip.finish().unwrap().into_inner())
}

/// F10.1B: a loan repayment row with interest and an external category id.
fn xlsx_repayment() -> String {
    let cuentas = vec![
        ["cuenta_id", "tipo", "nombre", "saldo_inicial"]
            .map(String::from)
            .to_vec(),
        ["bank", "banco", "Banco", "0"].map(String::from).to_vec(),
    ];
    let categorias = vec![
        ["categoria_id", "tipo", "nombre"]
            .map(String::from)
            .to_vec(),
        ["cat-ext", "gasto", "Intereses externos"]
            .map(String::from)
            .to_vec(),
    ];
    let movimientos = vec![
        [
            "movimiento_id",
            "tipo",
            "fecha",
            "cuenta_origen_id",
            "categoria_id",
            "monto",
            "interes",
        ]
        .map(String::from)
        .to_vec(),
        [
            "p1",
            "pago_prestamo",
            "2026-11-15",
            "bank",
            "cat-ext",
            "221",
            "21",
        ]
        .map(String::from)
        .to_vec(),
    ];
    package(&cuentas, &categorias, &movimientos)
}

/// Default workbook: one expense category and movements without related references.
fn xlsx(accounts: &[AccountRow], movements: &[MovementRow]) -> String {
    let refs: Vec<RefMovement> = movements
        .iter()
        .map(|(id, kind, date, account, amount)| {
            (*id, *kind, *date, *account, "com", *amount, None)
        })
        .collect();
    xlsx_full(accounts, &[("com", "gasto", "Comida")], &refs)
}

fn body(file: &str, token: Option<&str>, ack: Option<&str>) -> Value {
    body_with(file, token, ack, json!({}), json!({}), json!({}))
}

fn save_account(f: &Fixture, id: &str, kind: &str, initial: i64) {
    finapp::finance::save_account(
        &f.db,
        &f.user,
        &finapp::finance::Account {
            id: id.into(),
            name: id.into(),
            kind: kind.into(),
            initial_balance: initial,
        },
    )
    .unwrap();
}

fn save_category(f: &Fixture, id: &str, kind: &str) {
    finapp::finance::save_category(
        &f.db,
        &f.user,
        &finapp::finance::Category {
            id: id.into(),
            name: id.into(),
            kind: kind.into(),
        },
    )
    .unwrap();
}

fn apply_expense(f: &Fixture, id: &str, account: &str, category: &str, amount: i64, date: &str) {
    let change = finapp::finance::Change {
        request_id: id.into(),
        id: id.into(),
        expected_version: 0,
        deleted: false,
        kind: Some("expense".into()),
        account_id: Some(account.into()),
        category_id: Some(category.into()),
        card_account_id: None,
        destination_account_id: None,
        original_operation_id: None,
        amount: Some(amount),
        date: Some(date.into()),
        description: None,
        notes: None,
    };
    finapp::finance::apply(&f.db, &f.user, &change).unwrap();
}

fn count(f: &Fixture, sql: &str) -> i64 {
    f.db.0
        .lock()
        .unwrap()
        .query_row(sql, [&f.user], |r| r.get(0))
        .unwrap()
}

fn body_with(
    file: &str,
    token: Option<&str>,
    ack: Option<&str>,
    mapping: Value,
    category_mapping: Value,
    operation_mapping: Value,
) -> Value {
    let mut value = json!({
        "file":file, "mapping":mapping, "category_mapping":category_mapping,
        "operation_mapping":operation_mapping, "include_before_cutoff":true,
    });
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
async fn b01_external_account_links_to_internal() {
    let f = Fixture::new().await;
    save_account(&f, "banco", "bank", 1000);
    let file = xlsx(
        &[("src-bank", "banco", "Banco externo", 0, None, None)],
        &[("m1", "gasto", "2026-03-02", "src-bank", 100)],
    );
    let mut value = body_with(
        &file,
        None,
        None,
        json!({"src-bank":"banco"}),
        json!({}),
        json!({}),
    );
    let (status, report) = f.import(value.clone()).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["accounts"][0]["linked"], json!("banco"));
    let token = report["token"].as_str().unwrap().to_owned();
    value["preview_token"] = json!(token);
    let (status, done) = f.import(value).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(f.balance("banco"), 900);
    assert_eq!(
        count(&f, "SELECT COUNT(*) FROM finance_accounts WHERE user_id=?"),
        1
    );
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM import_sources WHERE user_id=? AND entity='account' AND source_id='src-bank' AND target_id='banco'"
        ),
        1
    );
    f.integrity();
}

#[tokio::test]
async fn b02_external_category_links_to_internal() {
    let f = Fixture::new().await;
    save_category(&f, "food", "expense");
    let file = xlsx_full(
        &[("banco", "banco", "Banco", 1000, None, None)],
        &[("cat-food", "gasto", "Comida externa")],
        &[("m1", "gasto", "2026-03-02", "banco", "cat-food", 100, None)],
    );
    let mut value = body_with(
        &file,
        None,
        None,
        json!({}),
        json!({"cat-food":"food"}),
        json!({}),
    );
    let (status, report) = f.import(value.clone()).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["categories"][0]["linked"], json!("food"));
    let token = report["token"].as_str().unwrap().to_owned();
    value["preview_token"] = json!(token);
    let (status, done) = f.import(value).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_categories WHERE user_id=?"
        ),
        1
    );
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND id='m1' AND category_id='food'"
        ),
        1
    );
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM import_sources WHERE user_id=? AND entity='category' AND source_id='cat-food' AND target_id='food'"
        ),
        1
    );
    f.integrity();
}

#[tokio::test]
async fn b03_external_category_is_created_when_unmapped() {
    let f = Fixture::new().await;
    let file = xlsx_full(
        &[("banco", "banco", "Banco", 1000, None, None)],
        &[("cat-food", "gasto", "Comida externa")],
        &[("m1", "gasto", "2026-03-02", "banco", "cat-food", 100, None)],
    );
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_categories WHERE user_id=? AND id='cat-food'"
        ),
        1
    );
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND id='m1' AND category_id='cat-food'"
        ),
        1
    );
    f.integrity();
}

#[tokio::test]
async fn b04_expense_and_refund_in_one_workbook() {
    let f = Fixture::new().await;
    let file = xlsx_full(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("com", "gasto", "Comida")],
        &[
            ("e1", "gasto", "2026-03-02", "banco", "com", 500, None),
            (
                "r1",
                "devolucion",
                "2026-03-03",
                "banco",
                "com",
                200,
                Some("e1"),
            ),
        ],
    );
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["errors"], json!([]));
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=?"
        ),
        2
    );
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND id='r1' AND kind='refund' AND original_operation_id='e1'"
        ),
        1
    );
    assert_eq!(f.balance("banco"), -300);
    f.integrity();
}

#[tokio::test]
async fn b04b_refund_before_expense_is_sorted_by_the_importer() {
    let f = Fixture::new().await;
    let file = xlsx_full(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("com", "gasto", "Comida")],
        &[
            (
                "r1",
                "devolucion",
                "2026-03-03",
                "banco",
                "com",
                200,
                Some("e1"),
            ),
            ("e1", "gasto", "2026-03-02", "banco", "com", 500, None),
        ],
    );
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["errors"], json!([]), "{report}");
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(f.balance("banco"), -300);
    f.integrity();
}

#[tokio::test]
async fn b05_refund_references_a_previously_imported_operation() {
    let f = Fixture::new().await;
    let first = xlsx(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("e1", "gasto", "2026-03-02", "banco", 500)],
    );
    let (_, preview) = f.import(body(&first, None, None)).await;
    let token = preview["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&first, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["committed"], json!(true));

    let second = xlsx_full(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("com", "gasto", "Comida")],
        &[(
            "r1",
            "devolucion",
            "2026-03-03",
            "banco",
            "com",
            200,
            Some("e1"),
        )],
    );
    let (_, preview) = f.import(body(&second, None, None)).await;
    let token = preview["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&second, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND id='r1' AND original_operation_id='e1'"
        ),
        1
    );
    assert_eq!(f.balance("banco"), -300);
    f.integrity();
}

#[tokio::test]
async fn b06_operation_mapping_resolves_an_internal_expense() {
    let f = Fixture::new().await;
    save_account(&f, "banco", "bank", 0);
    save_category(&f, "food", "expense");
    apply_expense(&f, "expense-existing", "banco", "food", 500, "2026-03-01");
    let file = xlsx_full(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("com", "gasto", "Comida")],
        &[(
            "r1",
            "devolucion",
            "2026-03-03",
            "banco",
            "food",
            200,
            Some("e1"),
        )],
    );
    let mut value = body_with(
        &file,
        None,
        None,
        json!({"banco":"banco"}),
        json!({}),
        json!({"e1":"expense-existing"}),
    );
    let (status, report) = f.import(value.clone()).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["errors"], json!([]), "{report}");
    let token = report["token"].as_str().unwrap().to_owned();
    value["preview_token"] = json!(token);
    let (status, done) = f.import(value).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND id='r1' AND original_operation_id='expense-existing'"
        ),
        1
    );
    f.integrity();
}

#[tokio::test]
async fn b06b_persisted_operation_link_survives_without_mapping() {
    let f = Fixture::new().await;
    save_account(&f, "banco", "bank", 0);
    save_category(&f, "food", "expense");
    apply_expense(&f, "expense-existing", "banco", "food", 500, "2026-03-01");
    let file = xlsx_full(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("com", "gasto", "Comida")],
        &[(
            "r1",
            "devolucion",
            "2026-03-03",
            "banco",
            "food",
            200,
            Some("e1"),
        )],
    );
    let mut value = body_with(
        &file,
        None,
        None,
        json!({"banco":"banco"}),
        json!({}),
        json!({"e1":"expense-existing"}),
    );
    let (_, preview) = f.import(value.clone()).await;
    let token = preview["token"].as_str().unwrap().to_owned();
    value["preview_token"] = json!(token);
    let (status, done) = f.import(value).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));

    let replay_file = xlsx_full(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("com", "gasto", "Comida")],
        &[(
            "r2",
            "devolucion",
            "2026-03-04",
            "banco",
            "food",
            200,
            Some("e1"),
        )],
    );
    let replay = body_with(
        &replay_file,
        None,
        None,
        json!({"banco":"banco"}),
        json!({}),
        json!({}),
    );
    let (status, report) = f.import(replay.clone()).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["errors"], json!([]), "{report}");
    let token = report["token"].as_str().unwrap().to_owned();
    let mut commit = replay;
    commit["preview_token"] = json!(token);
    let (status, done) = f.import(commit).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND id='r2' AND original_operation_id='expense-existing'"
        ),
        1
    );
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM import_sources WHERE user_id=? AND entity='operation' AND source_id='e1' AND target_id='expense-existing'"
        ),
        1
    );
    f.integrity();
}

#[tokio::test]
async fn b07_missing_reference_is_reported_and_blocks_commit() {
    let f = Fixture::new().await;
    let file = xlsx_full(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("com", "gasto", "Comida")],
        &[(
            "r1",
            "devolucion",
            "2026-03-03",
            "banco",
            "com",
            200,
            Some("missing"),
        )],
    );
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert!(!report["errors"].as_array().unwrap().is_empty());
    assert_eq!(
        report["references"],
        json!([{"source_id":"missing","movement_id":"r1"}])
    );
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, _) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=?"
        ),
        0
    );
    f.integrity();
}

#[tokio::test]
async fn b08_reference_to_another_user_is_rejected() {
    let f = Fixture::new().await;
    f.db.0
        .lock()
        .unwrap()
        .execute("INSERT INTO users VALUES('bob','bob','-',0)", [])
        .unwrap();
    finapp::finance::save_account(
        &f.db,
        "bob",
        &finapp::finance::Account {
            id: "banco-bob".into(),
            name: "Banco bob".into(),
            kind: "bank".into(),
            initial_balance: 0,
        },
    )
    .unwrap();
    finapp::finance::save_category(
        &f.db,
        "bob",
        &finapp::finance::Category {
            id: "food-bob".into(),
            name: "Food bob".into(),
            kind: "expense".into(),
        },
    )
    .unwrap();
    let change = finapp::finance::Change {
        request_id: "bob-expense".into(),
        id: "bob-expense".into(),
        expected_version: 0,
        deleted: false,
        kind: Some("expense".into()),
        account_id: Some("banco-bob".into()),
        category_id: Some("food-bob".into()),
        card_account_id: None,
        destination_account_id: None,
        original_operation_id: None,
        amount: Some(500),
        date: Some("2026-03-01".into()),
        description: None,
        notes: None,
    };
    finapp::finance::apply(&f.db, "bob", &change).unwrap();

    let file = xlsx_full(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("com", "gasto", "Comida")],
        &[(
            "r1",
            "devolucion",
            "2026-03-03",
            "banco",
            "com",
            200,
            Some("e1"),
        )],
    );
    let value = body_with(
        &file,
        None,
        None,
        json!({}),
        json!({}),
        json!({"e1":"bob-expense"}),
    );
    let (status, report) = f.import(value).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert!(!report["errors"].as_array().unwrap().is_empty());
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=?"
        ),
        0
    );
    f.integrity();
}

#[tokio::test]
async fn b09_same_source_id_in_different_entities_is_allowed() {
    let f = Fixture::new().await;
    let file = xlsx_full(
        &[("x", "banco", "Cuenta x", 0, None, None)],
        &[("x", "gasto", "Categoría x")],
        &[("x", "gasto", "2026-03-02", "x", "x", 100, None)],
    );
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["errors"], json!([]), "{report}");
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM import_sources WHERE user_id=? AND source_id='x'"
        ),
        3
    );
    f.integrity();
}

#[tokio::test]
async fn b10_identical_reimport_is_idempotent() {
    let f = Fixture::new().await;
    let file = xlsx(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("m1", "gasto", "2026-03-02", "banco", 100)],
    );
    let (_, preview) = f.import(body(&file, None, None)).await;
    let token = preview["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["committed"], json!(true));

    let (status, second) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["duplicates"], json!(3));
    assert_eq!(second["added"], json!(0));
    let token = second["token"].as_str().unwrap().to_owned();
    let (status, done_again) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done_again["committed"], json!(true));
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=?"
        ),
        1
    );
    assert_eq!(f.balance("banco"), -100);
    f.integrity();
}

#[tokio::test]
async fn b11_modified_row_is_detected_and_history_is_preserved() {
    let f = Fixture::new().await;
    let first = xlsx(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("m1", "gasto", "2026-03-02", "banco", 100)],
    );
    let (_, preview) = f.import(body(&first, None, None)).await;
    let token = preview["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&first, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["committed"], json!(true));

    let second = xlsx(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("m1", "gasto", "2026-03-02", "banco", 200)],
    );
    let (status, report) = f.import(body(&second, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert!(!report["errors"].as_array().unwrap().is_empty());
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, _) = f.import(body(&second, Some(&token), None)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND id='m1'"
        ),
        1
    );
    assert_eq!(f.balance("banco"), -100);
    f.integrity();
}

#[tokio::test]
async fn b12_persisted_category_link_cannot_be_redirected() {
    let f = Fixture::new().await;
    save_category(&f, "food", "expense");
    save_category(&f, "other", "expense");
    let file = xlsx_full(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("cat-food", "gasto", "Comida externa")],
        &[("m1", "gasto", "2026-03-02", "banco", "cat-food", 100, None)],
    );
    let mut value = body_with(
        &file,
        None,
        None,
        json!({}),
        json!({"cat-food":"food"}),
        json!({}),
    );
    let (_, preview) = f.import(value.clone()).await;
    let token = preview["token"].as_str().unwrap().to_owned();
    value["preview_token"] = json!(token);
    let (status, done) = f.import(value).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["committed"], json!(true));

    let mut redirect = body_with(
        &file,
        None,
        None,
        json!({}),
        json!({"cat-food":"other"}),
        json!({}),
    );
    let (status, report) = f.import(redirect.clone()).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert!(!report["errors"].as_array().unwrap().is_empty());
    let token = report["token"].as_str().unwrap().to_owned();
    redirect["preview_token"] = json!(token);
    let (status, _) = f.import(redirect).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM import_sources WHERE user_id=? AND entity='category' AND source_id='cat-food' AND target_id='food'"
        ),
        1
    );
    f.integrity();
}

#[tokio::test]
async fn b13_internal_duplicate_ids_block_the_import() {
    let f = Fixture::new().await;
    let file = xlsx_full(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("com", "gasto", "Comida")],
        &[
            ("m1", "gasto", "2026-03-02", "banco", "com", 100, None),
            ("m1", "gasto", "2026-03-03", "banco", "com", 100, None),
        ],
    );
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert!(!report["errors"].as_array().unwrap().is_empty());
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, _) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=?"
        ),
        0
    );
    f.integrity();
}

#[tokio::test]
async fn b14_archived_category_link_follows_current_import_rules() {
    let f = Fixture::new().await;
    save_category(&f, "old", "expense");
    f.db
        .0
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO catalog_details(user_id,entity,id,archived,institution,notes) VALUES(?1,'category','old',1,'','')",
            [&f.user],
        )
        .unwrap();
    let file = xlsx_full(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("cat-food", "gasto", "Comida externa")],
        &[("m1", "gasto", "2026-03-02", "banco", "cat-food", 100, None)],
    );
    let mut value = body_with(
        &file,
        None,
        None,
        json!({}),
        json!({"cat-food":"old"}),
        json!({}),
    );
    let (status, report) = f.import(value.clone()).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let token = report["token"].as_str().unwrap().to_owned();
    value["preview_token"] = json!(token);
    let (status, done) = f.import(value).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(
        done["committed"],
        json!(true),
        "current import rules allow archived categories"
    );
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND id='m1' AND category_id='old'"
        ),
        1
    );
    f.integrity();
}

#[tokio::test]
async fn b15_cutoff_movement_stays_out_of_the_balance() {
    let f = Fixture::new().await;
    let file = xlsx_full(
        &[("banco", "banco", "Banco", 1000, None, Some("2026-03-01"))],
        &[("com", "gasto", "Comida")],
        &[("m1", "gasto", "2026-02-15", "banco", "com", 100, None)],
    );
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert!(
        report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("anterior a la fecha de corte"))
    );
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(f.balance("banco"), 1000);
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND id='m1'"
        ),
        1
    );
    f.integrity();
}

#[tokio::test]
async fn b16_category_after_a_linked_account_resolves_correctly() {
    let f = Fixture::new().await;
    save_account(&f, "banco", "bank", 0);
    save_category(&f, "food", "expense");
    let file = xlsx_full(
        &[("src-bank", "banco", "Banco externo", 0, None, None)],
        &[("cat-food", "gasto", "Comida externa")],
        &[(
            "m1",
            "gasto",
            "2026-03-02",
            "src-bank",
            "cat-food",
            100,
            None,
        )],
    );
    let mut value = body_with(
        &file,
        None,
        None,
        json!({"src-bank":"banco"}),
        json!({"cat-food":"food"}),
        json!({}),
    );
    let (status, report) = f.import(value.clone()).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let token = report["token"].as_str().unwrap().to_owned();
    value["preview_token"] = json!(token);
    let (status, done) = f.import(value).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND id='m1' AND account_id='banco' AND category_id='food'"
        ),
        1
    );
    f.integrity();
}

#[tokio::test]
async fn b17_multiple_refunds_of_the_same_expense_are_allowed() {
    let f = Fixture::new().await;
    let file = xlsx_full(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("com", "gasto", "Comida")],
        &[
            ("e1", "gasto", "2026-03-02", "banco", "com", 500, None),
            (
                "r1",
                "devolucion",
                "2026-03-03",
                "banco",
                "com",
                200,
                Some("e1"),
            ),
            (
                "r2",
                "devolucion",
                "2026-03-04",
                "banco",
                "com",
                200,
                Some("e1"),
            ),
        ],
    );
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["errors"], json!([]), "{report}");
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(f.balance("banco"), -100);
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND kind='refund'"
        ),
        2
    );
    f.integrity();
}

#[tokio::test]
async fn b18_refunds_exceeding_the_expense_are_rejected() {
    let f = Fixture::new().await;
    let file = xlsx_full(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("com", "gasto", "Comida")],
        &[
            ("e1", "gasto", "2026-03-02", "banco", "com", 500, None),
            (
                "r1",
                "devolucion",
                "2026-03-03",
                "banco",
                "com",
                300,
                Some("e1"),
            ),
            (
                "r2",
                "devolucion",
                "2026-03-04",
                "banco",
                "com",
                300,
                Some("e1"),
            ),
        ],
    );
    let (status, report) = f.import(body(&file, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert!(!report["errors"].as_array().unwrap().is_empty());
    let token = report["token"].as_str().unwrap().to_owned();
    let (status, _) = f.import(body(&file, Some(&token), None)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=?"
        ),
        0
    );
    f.integrity();
}

#[tokio::test]
async fn b19_mapping_changed_after_preview_is_rejected() {
    let f = Fixture::new().await;
    save_account(&f, "banco", "bank", 0);
    save_category(&f, "food", "expense");
    save_category(&f, "other", "expense");
    let file = xlsx_full(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("cat-food", "gasto", "Comida externa")],
        &[("m1", "gasto", "2026-03-02", "banco", "cat-food", 100, None)],
    );
    let preview = body_with(
        &file,
        None,
        None,
        json!({}),
        json!({"cat-food":"food"}),
        json!({}),
    );
    let (_, report) = f.import(preview).await;
    let token = report["token"].as_str().unwrap().to_owned();
    let confirm = body_with(
        &file,
        Some(&token),
        None,
        json!({}),
        json!({"cat-food":"other"}),
        json!({}),
    );
    let (status, _) = f.import(confirm).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=?"
        ),
        0
    );
    f.integrity();
}

#[tokio::test]
async fn b19b_unused_operation_mapping_still_binds_the_token() {
    let f = Fixture::new().await;
    save_account(&f, "banco", "bank", 0);
    save_category(&f, "food", "expense");
    apply_expense(&f, "expense-a", "banco", "food", 100, "2026-03-01");
    apply_expense(&f, "expense-b", "banco", "food", 100, "2026-03-02");
    let file = xlsx_full(
        &[("banco", "banco", "Banco", 0, None, None)],
        &[("com", "gasto", "Comida")],
        &[],
    );
    let preview = body_with(
        &file,
        None,
        None,
        json!({"banco":"banco"}),
        json!({}),
        json!({"unused":"expense-a"}),
    );
    let (_, report) = f.import(preview).await;
    let token = report["token"].as_str().unwrap().to_owned();
    let confirm = body_with(
        &file,
        Some(&token),
        None,
        json!({"banco":"banco"}),
        json!({}),
        json!({"unused":"expense-b"}),
    );
    let (status, _) = f.import(confirm).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND id LIKE 'm%'"
        ),
        0
    );
    f.integrity();
}

#[tokio::test]
async fn b20_large_workbook_with_mappings_imports_cleanly() {
    let f = Fixture::new().await;
    save_account(&f, "banco", "bank", 0);
    save_category(&f, "food", "expense");
    let file = large_xlsx();
    let mut value = body_with(
        &file,
        None,
        None,
        json!({"src-bank":"banco"}),
        json!({"cat-food":"food"}),
        json!({}),
    );
    let (status, report) = f.import(value.clone()).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["errors"], json!([]), "{report}");
    assert_eq!(report["added"], json!(25_002));
    let token = report["token"].as_str().unwrap().to_owned();
    value["preview_token"] = json!(token);
    let (status, done) = f.import(value).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND category_id='food'"
        ),
        25_000
    );
    f.integrity();
}

fn large_xlsx() -> String {
    const MOVEMENTS: usize = 25_000;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let content_types = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/></Types>"#;
    let book = r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Cuentas" sheetId="1" r:id="rId1"/><sheet name="Categorias" sheetId="2" r:id="rId2"/><sheet name="Movimientos" sheetId="3" r:id="rId3"/></sheets></workbook>"#;
    let rel = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet3.xml"/></Relationships>"#;
    let shared = r#"<sst count="6" uniqueCount="6"><si><t>gasto</t></si><si><t>2026-03-02</t></si><si><t>src-bank</t></si><si><t>cat-food</t></si><si><t>1</t></si><si><t>banco</t></si></sst>"#;
    fn inline(value: &str) -> String {
        format!(r#"<c t="inlineStr"><is><t>{value}</t></is></c>"#)
    }
    fn shared_cell(index: usize) -> String {
        format!(r#"<c t="s"><v>{index}</v></c>"#)
    }
    fn sheet(rows: Vec<String>) -> String {
        let mut xml = String::from(
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
        );
        for row in rows {
            xml.push_str("<row>");
            xml.push_str(&row);
            xml.push_str("</row>");
        }
        xml.push_str("</sheetData></worksheet>");
        xml
    }
    let cuentas = sheet(vec![
        inline("cuenta_id") + &inline("tipo") + &inline("nombre") + &inline("saldo_inicial"),
        inline("src-bank") + &inline("banco") + &inline("Banco externo") + &inline("0"),
    ]);
    let categorias = sheet(vec![
        inline("categoria_id") + &inline("tipo") + &inline("nombre"),
        inline("cat-food") + &inline("gasto") + &inline("Comida externa"),
    ]);
    let mut movimientos = String::from(
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
    );
    movimientos.push_str(
        &("<row>".to_owned()
            + &inline("movimiento_id")
            + &inline("tipo")
            + &inline("fecha")
            + &inline("cuenta_origen_id")
            + &inline("categoria_id")
            + &inline("monto")
            + &inline("movimiento_relacionado_id")
            + "</row>"),
    );
    for index in 0..MOVEMENTS {
        movimientos.push_str("<row>");
        movimientos.push_str(&inline(&format!("m{index}")));
        movimientos.push_str(&shared_cell(0));
        movimientos.push_str(&shared_cell(1));
        movimientos.push_str(&shared_cell(2));
        movimientos.push_str(&shared_cell(3));
        movimientos.push_str(&shared_cell(4));
        movimientos.push_str(&shared_cell(5));
        movimientos.push_str("</row>");
    }
    movimientos.push_str("</sheetData></worksheet>");
    for (name, content) in [
        ("_rels/.rels", PACKAGE_RELS),
        ("[Content_Types].xml", content_types),
        ("xl/workbook.xml", book),
        ("xl/_rels/workbook.xml.rels", rel),
        ("xl/sharedStrings.xml", shared),
        ("xl/worksheets/sheet1.xml", &cuentas),
        ("xl/worksheets/sheet2.xml", &categorias),
        ("xl/worksheets/sheet3.xml", &movimientos),
    ] {
        zip.start_file(name, options).unwrap();
        std::io::Write::write_all(&mut zip, content.as_bytes()).unwrap();
    }
    STANDARD.encode(zip.finish().unwrap().into_inner())
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

/// F10.1B: the interest part of an imported loan repayment honors the linked category.
#[tokio::test]
async fn f10_loan_repayment_interest_uses_the_linked_category() {
    let f = Fixture::new().await;
    save_category(&f, "interest", "expense");
    let file = xlsx_repayment();
    let mut value = body_with(
        &file,
        None,
        None,
        json!({}),
        json!({"cat-ext": "interest"}),
        json!({}),
    );
    let (status, report) = f.import(value.clone()).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");
    let token = report["token"].as_str().unwrap().to_owned();
    value["preview_token"] = json!(token);
    let (status, done) = f.import(value).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=?"
        ),
        2
    );
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND id='p1' AND kind='loan_repayment' AND amount=200"
        ),
        1
    );
    assert_eq!(
        count(
            &f,
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=? AND kind='expense' AND category_id='interest' AND amount=21"
        ),
        1
    );
    assert_eq!(f.balance("bank"), -221);
    f.integrity();
}
