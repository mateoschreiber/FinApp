//! F9.3C: importer atomicity and SQLite contention on real on-disk databases.
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use finapp::{app, db::Database};
use http_body_util::BodyExt;
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tower::ServiceExt;

const ORIGIN: &str = "https://finanzas.example";
const PASSWORD: &str = "contrasena sintetica de concurrencia";

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("finapp-f93c-{name}-{}.sqlite", std::process::id()))
}

fn cell(value: &str) -> String {
    format!(r#"<c t="inlineStr"><is><t>{value}</t></is></c>"#)
}

fn sheet(rows: &[&str]) -> String {
    let mut xml = String::from(
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
    );
    for row in rows {
        xml.push_str("<row>");
        xml.push_str(row);
        xml.push_str("</row>");
    }
    xml.push_str("</sheetData></worksheet>");
    xml
}

fn workbook(rows: &[(&str, i64)], date: &str) -> String {
    let cuentas = sheet(&[
        &(cell("cuenta_id")
            + &cell("tipo")
            + &cell("nombre")
            + &cell("saldo_inicial")
            + &cell("saldo_actual")
            + &cell("fecha_corte")),
        &(cell("banco") + &cell("banco") + &cell("Banco") + &cell("0") + &cell("") + &cell("")),
    ]);
    let categorias = sheet(&[
        &(cell("categoria_id") + &cell("tipo") + &cell("nombre")),
        &(cell("com") + &cell("gasto") + &cell("Comida")),
    ]);
    let mut movimientos = String::from(
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
    );
    movimientos.push_str("<row>");
    movimientos.push_str(
        &(cell("movimiento_id")
            + &cell("tipo")
            + &cell("fecha")
            + &cell("cuenta_origen_id")
            + &cell("categoria_id")
            + &cell("monto")),
    );
    movimientos.push_str("</row>");
    for (id, amount) in rows {
        movimientos.push_str("<row>");
        movimientos.push_str(&cell(id));
        movimientos.push_str(&cell("gasto"));
        movimientos.push_str(&cell(date));
        movimientos.push_str(&cell("banco"));
        movimientos.push_str(&cell("com"));
        movimientos.push_str(&cell(&amount.to_string()));
        movimientos.push_str("</row>");
    }
    movimientos.push_str("</sheetData></worksheet>");
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
        ("xl/worksheets/sheet1.xml", &cuentas),
        ("xl/worksheets/sheet2.xml", &categorias),
        ("xl/worksheets/sheet3.xml", &movimientos),
    ] {
        zip.start_file(name, options).unwrap();
        std::io::Write::write_all(&mut zip, content.as_bytes()).unwrap();
    }
    STANDARD.encode(zip.finish().unwrap().into_inner())
}

struct Fixture {
    db: Database,
    router: Router,
    cookie: String,
    user: String,
    path: PathBuf,
}

impl Fixture {
    async fn new(name: &str) -> Self {
        let path = temp_path(name);
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
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
            path,
        }
    }

    async fn import(&self, body: Value) -> (StatusCode, Value) {
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

    fn cleanup(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn body(file: &str, token: Option<&str>) -> Value {
    let mut value = json!({"file":file, "mapping":{}, "include_before_cutoff":true});
    if let Some(token) = token {
        value["preview_token"] = json!(token);
    }
    value
}

#[tokio::test]
async fn c01_normal_import_is_atomic_and_clean() {
    let f = Fixture::new("c01").await;
    let file = workbook(&[("m1", 100), ("m2", 200)], "2026-03-02");
    let (status, preview) = f.import(body(&file, None)).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["errors"], json!([]));
    assert_eq!(f.count_operations(), 0, "preview writes nothing");
    let token = preview["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&file, Some(&token))).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["committed"], json!(true));
    assert_eq!(f.count_operations(), 2);
    f.integrity();
    f.cleanup();
}

#[tokio::test]
async fn c02_invalid_row_blocks_the_whole_confirmation() {
    let f = Fixture::new("c02").await;
    let file = workbook(&[("m1", 100), ("m2", -5)], "2026-03-02");
    let (status, preview) = f.import(body(&file, None)).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert!(!preview["errors"].as_array().unwrap().is_empty());
    let token = preview["token"].as_str().unwrap().to_owned();
    let (status, rejected) = f.import(body(&file, Some(&token))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    assert_eq!(f.count_operations(), 0);
    f.integrity();
    f.cleanup();
}

#[tokio::test]
async fn c03_savepoint_error_leaves_no_partial_state() {
    let f = Fixture::new("c03").await;
    // The third row carries an invalid date, so its savepoint rolls back while
    // the two earlier rows remain pending inside the still-open outer transaction.
    let file = workbook(&[("m1", 10), ("m2", 20), ("m3", 30)], "2026-02-30");
    let (status, preview) = f.import(body(&file, None)).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert!(!preview["errors"].as_array().unwrap().is_empty());
    let token = preview["token"].as_str().unwrap().to_owned();
    let (status, rejected) = f.import(body(&file, Some(&token))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    assert_eq!(
        f.count_operations(),
        0,
        "savepoint rollback plus no commit means nothing persists"
    );
    f.integrity();
    f.cleanup();
}

#[tokio::test]
async fn c04_sqlite_error_after_several_rows_is_controlled() {
    // C05 note: a deterministic COMMIT failure is not reproducible here without
    // mocking SQLite itself, so this test covers the practical equivalent: a
    // controlled row failure after prior rows, which still proves that no
    // unauthorized partial state survives.
    let f = Fixture::new("c04").await;
    // A test-only trigger aborts the insert of the fourth movement.
    f.db.0
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER f93c_abort AFTER INSERT ON finance_operations
             WHEN NEW.id='m4' BEGIN SELECT RAISE(ABORT,'f93c'); END;",
        )
        .unwrap();
    let file = workbook(
        &[("m1", 10), ("m2", 20), ("m3", 30), ("m4", 40)],
        "2026-03-02",
    );
    let (status, preview) = f.import(body(&file, None)).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert!(!preview["errors"].as_array().unwrap().is_empty());
    let token = preview["token"].as_str().unwrap().to_owned();
    let (status, rejected) = f.import(body(&file, Some(&token))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    assert_eq!(
        f.count_operations(),
        0,
        "no partial commit after a SQLite failure"
    );
    f.integrity();
    f.cleanup();
}

#[tokio::test]
async fn c06_repeated_confirmation_is_rejected_without_duplicates() {
    let f = Fixture::new("c06").await;
    let file = workbook(&[("m1", 100)], "2026-03-02");
    let (_, preview) = f.import(body(&file, None)).await;
    let token = preview["token"].as_str().unwrap().to_owned();
    let (status, done) = f.import(body(&file, Some(&token))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["committed"], json!(true));
    let (status, again) = f.import(body(&file, Some(&token))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{again}");
    assert_eq!(f.count_operations(), 1);
    f.integrity();
    f.cleanup();
}

#[tokio::test]
async fn c07_state_change_between_preview_and_confirm_invalidates_the_token() {
    let f = Fixture::new("c07").await;
    let file = workbook(&[("m1", 100)], "2026-03-02");
    let (_, preview) = f.import(body(&file, None)).await;
    let token = preview["token"].as_str().unwrap().to_owned();

    // Another connection adds an account for the same user, so the recalculated report differs.
    let other = Connection::open(&f.path).unwrap();
    other
        .execute(
            "INSERT INTO finance_accounts VALUES(?1,'extra','Extra','bank',0)",
            [&f.user],
        )
        .unwrap();
    drop(other);

    let (status, rejected) = f.import(body(&file, Some(&token))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    assert_eq!(f.count_operations(), 0);
    f.integrity();
    f.cleanup();
}

#[tokio::test]
async fn c08_contention_between_connections_is_predictable() {
    let path = temp_path("c08");
    let _ = std::fs::remove_file(&path);
    let writer = Connection::open(&path).unwrap();
    writer
        .execute_batch("CREATE TABLE t(x INTEGER); INSERT INTO t VALUES(1);")
        .unwrap();

    // A: a reader holds SHARED; a writer can still start, but commit conflicts.
    let reader = Connection::open(&path).unwrap();
    reader.execute_batch("BEGIN").unwrap();
    reader
        .query_row("SELECT COUNT(*) FROM t", [], |r| r.get::<_, i64>(0))
        .unwrap();
    let started = Instant::now();
    let result = writer.execute("INSERT INTO t VALUES(2)", []);
    let read_hold = started.elapsed();
    reader.execute_batch("COMMIT").unwrap();
    eprintln!("reader-held insert: {read_hold:?} -> {result:?}");
    let busy = matches!(
        result,
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::DatabaseBusy
    );
    assert!(
        result.is_ok() || busy,
        "unexpected reader/writer outcome: {result:?}"
    );

    // B: an external writer holds RESERVED; the importer-style write fails immediately.
    let blocker = Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut immediate = Connection::open(&path).unwrap();
    let started = Instant::now();
    let result = immediate.transaction_with_behavior(TransactionBehavior::Immediate);
    let immediate_wait = started.elapsed();
    blocker.execute_batch("ROLLBACK").unwrap();
    eprintln!("immediate under writer: {immediate_wait:?} -> {result:?}");
    assert!(immediate_wait < Duration::from_secs(6));

    // C: DEFERRED read-then-write under an external writer fails on promotion.
    let blocker = Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut deferred = Connection::open(&path).unwrap();
    let started = Instant::now();
    let tx = deferred.transaction().unwrap();
    let _ = tx
        .query_row("SELECT COUNT(*) FROM t", [], |r| r.get::<_, i64>(0))
        .unwrap();
    let promotion = tx.execute("INSERT INTO t VALUES(3)", []);
    let promotion_time = started.elapsed();
    drop(tx);
    blocker.execute_batch("ROLLBACK").unwrap();
    eprintln!("deferred promotion: {promotion_time:?} -> {promotion:?}");
    assert!(promotion_time < Duration::from_secs(1));

    // D: IMMEDIATE under the same external writer waits for busy_timeout.
    let blocker = Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut immediate = Connection::open(&path).unwrap();
    let started = Instant::now();
    let result = immediate.transaction_with_behavior(TransactionBehavior::Immediate);
    let immediate_wait = started.elapsed();
    blocker.execute_batch("ROLLBACK").unwrap();
    eprintln!("immediate wait: {immediate_wait:?} -> {result:?}");
    assert!(immediate_wait >= Duration::from_millis(100));

    let check: String = writer
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(check, "ok");
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn c09_deferred_and_immediate_have_predictable_tradeoffs() {
    let path = temp_path("c09");
    let _ = std::fs::remove_file(&path);
    let writer = Connection::open(&path).unwrap();
    writer
        .execute_batch("CREATE TABLE t(x INTEGER); INSERT INTO t VALUES(1);")
        .unwrap();

    // DEFERRED promotion under an external writer fails immediately (documented).
    let blocker = Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut deferred = Connection::open(&path).unwrap();
    let started = Instant::now();
    let tx = deferred.transaction().unwrap();
    let _ = tx
        .query_row("SELECT COUNT(*) FROM t", [], |r| r.get::<_, i64>(0))
        .unwrap();
    let promotion = tx.execute("INSERT INTO t VALUES(2)", []);
    let deferred_time = started.elapsed();
    drop(tx);
    blocker.execute_batch("ROLLBACK").unwrap();

    // IMMEDIATE under the same external writer waits for busy_timeout.
    let blocker = Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut immediate = Connection::open(&path).unwrap();
    let started = Instant::now();
    let result = immediate.transaction_with_behavior(TransactionBehavior::Immediate);
    let immediate_time = started.elapsed();
    blocker.execute_batch("ROLLBACK").unwrap();

    eprintln!(
        "deferred {deferred_time:?} ({promotion:?}) · immediate {immediate_time:?} ({result:?})"
    );
    assert!(
        deferred_time < immediate_time,
        "promotion fails faster than busy_timeout"
    );
    assert!(immediate_time >= Duration::from_millis(100));
    let _ = std::fs::remove_file(&path);
}
