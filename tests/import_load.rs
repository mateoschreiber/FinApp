use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use finapp::{app, db::Database};
use http_body_util::BodyExt;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tower::ServiceExt;

const ORIGIN: &str = "https://finanzas.example";
const PASSWORD: &str = "contrasena sintetica de carga";
const MOVEMENTS: usize = 25_000;

// A stored (uncompressed) workbook must stay under the 5 MiB upload limit, so data
// cells use shared strings and only the unique movement ids stay inline.
fn workbook(movements: usize) -> String {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let content_types = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/></Types>"#;
    let book = r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Cuentas" sheetId="1" r:id="rId1"/><sheet name="Categorias" sheetId="2" r:id="rId2"/><sheet name="Movimientos" sheetId="3" r:id="rId3"/></sheets></workbook>"#;
    let rel = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet3.xml"/></Relationships>"#;
    let shared_strings = r#"<sst count="6" uniqueCount="5"><si><t>gasto</t></si><si><t>2026-03-02</t></si><si><t>carga</t></si><si><t>com</t></si><si><t>1</t></si></sst>"#;
    fn inline(value: &str) -> String {
        format!(r#"<c t="inlineStr"><is><t>{value}</t></is></c>"#)
    }
    fn shared(index: usize) -> String {
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
        inline("cuenta_id") + &inline("tipo") + &inline("nombre"),
        inline("carga") + &inline("banco") + &inline("Carga"),
    ]);
    let categorias = sheet(vec![
        inline("categoria_id") + &inline("tipo") + &inline("nombre"),
        inline("com") + &inline("gasto") + &inline("Comida"),
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
            + "</row>"),
    );
    for index in 0..movements {
        movimientos.push_str("<row>");
        movimientos.push_str(&inline(&format!("m{index}")));
        movimientos.push_str(&shared(0));
        movimientos.push_str(&shared(1));
        movimientos.push_str(&shared(2));
        movimientos.push_str(&shared(3));
        movimientos.push_str(&shared(4));
        movimientos.push_str("</row>");
    }
    movimientos.push_str("</sheetData></worksheet>");
    for (name, content) in [
        ("[Content_Types].xml", content_types),
        ("xl/workbook.xml", book),
        ("xl/_rels/workbook.xml.rels", rel),
        ("xl/sharedStrings.xml", shared_strings),
        ("xl/worksheets/sheet1.xml", &cuentas),
        ("xl/worksheets/sheet2.xml", &categorias),
        ("xl/worksheets/sheet3.xml", &movimientos),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    STANDARD.encode(zip.finish().unwrap().into_inner())
}

fn post_import(value: &Value, cookie: &str, user: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/import/xlsx")
        .header("origin", ORIGIN)
        .header("content-type", "application/json")
        .header(header::COOKIE, cookie)
        .header("x-finapp-user", user)
        .body(Body::from(value.to_string()))
        .unwrap()
}

/// Measures the 25k-row import the code accepts, plus what a concurrent user and the
/// Docker healthcheck experience while it runs. It only asserts loose bounds; the
/// exact numbers are printed and belong in the phase report.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn importing_25k_rows_measures_the_shared_connection_lock() {
    let db = Database::open_memory().unwrap();
    db.bootstrap_admin("alice", PASSWORD).unwrap();
    let router = app(db.clone(), ORIGIN.into());
    let login = router
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
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = login.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let user: Value =
        serde_json::from_slice(&login.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let user_id = user["id"].as_str().unwrap().to_owned();

    let file = workbook(MOVEMENTS);
    let preview_body = json!({"file":file});
    let preview_started = Instant::now();
    let response = router
        .clone()
        .oneshot(post_import(&preview_body, &cookie, &user_id))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let report: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(report["added"], json!(MOVEMENTS + 2), "preview: {report}");
    assert_eq!(report["errors"], json!([]), "preview: {report}");
    let preview_time = preview_started.elapsed();

    let done = Arc::new(AtomicBool::new(false));
    let health_done = Arc::clone(&done);
    let health_router = router.clone();
    let health = tokio::spawn(async move {
        let mut worst = Duration::ZERO;
        while !health_done.load(Ordering::Relaxed) {
            let started = Instant::now();
            let response = health_router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/health")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            worst = worst.max(started.elapsed());
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        worst
    });

    let started = Instant::now();
    let mut confirm_body = preview_body.clone();
    confirm_body["preview_token"] = report["token"].clone();
    let import_router = router.clone();
    let import_request = post_import(&confirm_body, &cookie, &user_id);
    let confirm = tokio::spawn(async move {
        let response = import_router.oneshot(import_request).await.unwrap();
        let status = response.status();
        let body: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        (status, body["committed"] == json!(true), started.elapsed())
    });
    // Let the import take the shared connection before firing a concurrent reader;
    // the first half of the preview pass is the XLSX parse, the rest holds the lock.
    tokio::time::sleep(preview_time / 2).await;
    let snapshot_router = router.clone();
    let snapshot_cookie = cookie.clone();
    let snapshot_user = user_id.clone();
    let snapshot = tokio::spawn(async move {
        let began = Instant::now();
        let response = snapshot_router
            .oneshot(
                Request::builder()
                    .uri("/api/snapshot")
                    .header(header::COOKIE, &snapshot_cookie)
                    .header("x-finapp-user", &snapshot_user)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        (response.status(), began.elapsed())
    });

    let (status, committed, import_time) = confirm.await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(committed, json!(true));
    done.store(true, Ordering::Relaxed);
    let worst_health = health.await.unwrap();
    let (snapshot_status, snapshot_latency) = snapshot.await.unwrap();

    eprintln!(
        "25k import (preview+confirm): {import_time:?} · worst /health during import: {worst_health:?} · concurrent snapshot: {snapshot_status} after {snapshot_latency:?}"
    );
    assert_eq!(snapshot_status, StatusCode::OK);
    assert!(
        import_time < Duration::from_secs(300),
        "25k import took {import_time:?}"
    );
    assert!(
        worst_health < Duration::from_secs(30),
        "/health stalled {worst_health:?} during the import"
    );

    // C10: exact persisted counts and structural integrity after the 25k import.
    {
        let conn = db.0.lock().unwrap();
        let operations: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM finance_operations WHERE user_id=?1",
                [&user_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(operations, MOVEMENTS as i64);
        let sources: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM import_sources WHERE user_id=?1",
                [&user_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sources, MOVEMENTS as i64 + 2);
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

async fn login_user(router: &Router) -> (String, String) {
    let login = router
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
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = login.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let user: Value =
        serde_json::from_slice(&login.into_body().collect().await.unwrap().to_bytes()).unwrap();
    (cookie, user["id"].as_str().unwrap().to_owned())
}

async fn report_of(router: &Router, body: &Value, cookie: &str, user: &str) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(post_import(body, cookie, user))
        .await
        .unwrap();
    let status = response.status();
    let report =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    (status, report)
}

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("finapp-f84-{name}-{}.sqlite", std::process::id()))
}

/// A second connection takes a write lock before the import writes. The 5 s busy
/// timeout expires and the import reports a row error instead of hanging or failing
/// silently. The public message must stay the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controlled_write_lock_reports_a_row_error() {
    let path = temp_path("lock");
    let _ = std::fs::remove_file(&path);
    let db = Database::open(&path).unwrap();
    db.bootstrap_admin("alice", PASSWORD).unwrap();
    let router = app(db, ORIGIN.into());
    let (cookie, user) = login_user(&router).await;

    let body = json!({"file": workbook(2)});
    let (status, preview) = report_of(&router, &body, &cookie, &user).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preview["errors"], json!([]), "preview: {preview}");

    let blocker = Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = Instant::now();
    let mut confirm = body.clone();
    confirm["preview_token"] = preview["token"].clone();
    let response = router
        .clone()
        .oneshot(post_import(&confirm, &cookie, &user))
        .await
        .unwrap();
    let status = response.status();
    let public_message =
        String::from_utf8_lossy(&response.into_body().collect().await.unwrap().to_bytes())
            .to_string();
    let elapsed = started.elapsed();
    blocker.execute_batch("ROLLBACK").unwrap();
    drop(blocker);

    // A row error makes the confirmation reject the stale preview with the same
    // public message as before. The SQLite code is in stderr, not in the response.
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        public_message.contains("La vista previa cambió"),
        "{public_message}"
    );
    eprintln!("controlled lock: {elapsed:?} · public message: {public_message}");
    // SQLite returns BUSY immediately when a DEFERRED transaction must promote a
    // read lock to a write lock while another connection holds RESERVED. The 5 s
    // busy timeout is not invoked in that case, so no minimum wait is asserted.

    let check: String = Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(check, "ok");
    let _ = std::fs::remove_file(&path);
}

/// Removing the source table under a second connection produces a deterministic
/// SQLite failure. The public message stays compatible and the row is not committed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sqlite_failure_surface_is_public_and_transaction_stays_clean() {
    let path = temp_path("schema");
    let _ = std::fs::remove_file(&path);
    let db = Database::open(&path).unwrap();
    db.bootstrap_admin("alice", PASSWORD).unwrap();
    let router = app(db.clone(), ORIGIN.into());
    let (cookie, user) = login_user(&router).await;
    Connection::open(&path)
        .unwrap()
        .execute_batch("DROP TABLE import_sources")
        .unwrap();

    let body = json!({"file": workbook(2)});
    let (status, report) = report_of(&router, &body, &cookie, &user).await;
    assert_eq!(status, StatusCode::OK);
    assert!(report["errors"].as_array().is_some_and(|e| !e.is_empty()));
    assert!(
        report["errors"][0]
            .as_str()
            .unwrap()
            .contains("No se pudo consultar el historial de importación.")
    );
    assert_eq!(report["committed"], json!(false));
    let conn = db.0.lock().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=?1",
            [&user],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "a failed row must leave no operation behind");
    drop(conn);
    let _ = std::fs::remove_file(&path);
}

/// The same workbook confirmed twice must not duplicate movements.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reimporting_the_same_workbook_is_idempotent() {
    let db = Database::open_memory().unwrap();
    db.bootstrap_admin("alice", PASSWORD).unwrap();
    let router = app(db.clone(), ORIGIN.into());
    let (cookie, user) = login_user(&router).await;
    let body = json!({"file": workbook(2)});

    let (status, first) = report_of(&router, &body, &cookie, &user).await;
    assert_eq!(status, StatusCode::OK);
    let mut done = body.clone();
    done["preview_token"] = first["token"].clone();
    let (status, first_done) = report_of(&router, &done, &cookie, &user).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first_done["committed"], json!(true));

    let (status, second) = report_of(&router, &body, &cookie, &user).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["duplicates"], json!(4), "second pass: {second}");
    assert_eq!(second["added"], json!(0), "second pass: {second}");
    assert_eq!(second["errors"], json!([]), "second pass: {second}");
    let mut done_again = body.clone();
    done_again["preview_token"] = second["token"].clone();
    let (status, second_done) = report_of(&router, &done_again, &cookie, &user).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second_done["committed"], json!(true));

    let conn = db.0.lock().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM finance_operations WHERE user_id=?1",
            [&user],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2, "no duplicated movements after a reimport");
}
