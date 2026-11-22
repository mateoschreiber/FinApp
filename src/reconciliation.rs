//! Compare immutable bank statement lines to signed ledger postings.
use crate::{AppState, finance};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

type Problem = (StatusCode, String);
type Result<T> = std::result::Result<T, Problem>;
fn invalid(msg: impl Into<String>) -> Problem {
    (StatusCode::BAD_REQUEST, msg.into())
}
fn conflict(msg: impl Into<String>) -> Problem {
    (StatusCode::CONFLICT, msg.into())
}
fn database(_: rusqlite::Error) -> Problem {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "Error al leer la conciliación.".into(),
    )
}
fn owner(s: &AppState, h: &HeaderMap) -> Result<String> {
    finance::bound_user(&s.db, h)
        .map(|u| u.id)
        .map_err(|code| (code, "La sesión cambió.".into()))
}
fn account_exists(c: &Connection, user: &str, id: &str) -> Result<()> {
    let exists: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM finance_accounts WHERE user_id=?1 AND id=?2)",
            params![user, id],
            |r| r.get(0),
        )
        .map_err(database)?;
    if !exists {
        return Err((StatusCode::NOT_FOUND, "Cuenta inexistente.".into()));
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
struct InputLine {
    id: String,
    date: String,
    amount: i64,
    description: String,
    reference: Option<String>,
}

fn parse_csv(csv: &str) -> Result<Vec<InputLine>> {
    if csv.len() > 1024 * 1024 {
        return Err(invalid("El extracto supera 1 MiB."));
    }
    let content = csv.trim_start_matches('\u{feff}');
    // Try each delimiter on the actual header so quoted delimiters in a header remain valid.
    for delimiter in b",;\t" {
        let mut reader = csv::ReaderBuilder::new()
            .delimiter(*delimiter)
            .trim(csv::Trim::All)
            .from_reader(content.as_bytes());
        let names: Vec<_> = reader
            .headers()
            .map_err(|_| invalid("Cabecera CSV inválida."))?
            .iter()
            .map(|s| s.to_lowercase().replace('ó', "o"))
            .collect();
        let find = |aliases: &[&str]| names.iter().position(|s| aliases.contains(&s.as_str()));
        let (Some(date_col), Some(amount_col)) = (
            find(&["fecha", "date"]),
            find(&["importe", "monto", "amount"]),
        ) else {
            continue;
        };
        let desc_col = find(&["descripcion", "concepto", "description"]);
        let ref_col = find(&["referencia", "id", "reference"]);
        let mut result = Vec::new();
        let mut ids = HashSet::new();
        for (index, row) in reader.records().enumerate() {
            let row_no = index + 2;
            if index >= 5000 {
                return Err(invalid("Se admiten hasta 5000 filas por extracto."));
            }
            let row = row.map_err(|_| invalid(format!("Fila {row_no}: columnas inválidas.")))?;
            let date = &row[date_col];
            let amount = row[amount_col]
                .parse::<i64>()
                .ok()
                .filter(|v| *v != 0 && v.unsigned_abs() <= 9_007_199_254_740_991)
                .ok_or_else(|| {
                    invalid(format!(
                        "Fila {row_no}: el importe debe ser un entero con signo en guaraníes."
                    ))
                })?;
            if !finance::valid_date(date) {
                return Err(invalid(format!("Fila {row_no}: usa fecha AAAA-MM-DD.")));
            }
            let description = desc_col.map(|i| &row[i]).unwrap_or("");
            let reference = ref_col.map(|i| &row[i]).filter(|r| !r.is_empty());
            if description.len() > 1000 || reference.is_some_and(|s| s.len() > 256) {
                return Err(invalid(format!(
                    "Fila {row_no}: descripción o referencia demasiado larga."
                )));
            }
            let id = if let Some(reference) = reference {
                format!("ref:{reference}")
            } else {
                let signature = serde_json::to_vec(&(date, amount, description))
                    .expect("serializable CSV values");
                format!("hash:{}", hex::encode(Sha256::digest(signature)))
            };
            if !ids.insert(id.clone()) {
                return Err(invalid(format!(
                    "Fila {row_no}: referencia repetida. Asigna una referencia única a las líneas iguales."
                )));
            }
            result.push(InputLine {
                id,
                date: date.into(),
                amount,
                description: description.into(),
                reference: reference.map(str::to_owned),
            });
        }
        if result.is_empty() {
            return Err(invalid("El extracto está vacío."));
        }
        return Ok(result);
    }
    Err(invalid("La cabecera debe incluir fecha e importe."))
}

#[derive(Deserialize)]
struct Upload {
    account_id: String,
    csv: String,
    #[serde(default)]
    confirm: bool,
}
#[derive(Debug, Serialize)]
struct Imported {
    added: usize,
    existing: usize,
    committed: bool,
}
fn import(c: &mut Connection, user: &str, input: &Upload) -> Result<Imported> {
    let lines = parse_csv(&input.csv)?;
    let tx = c.transaction().map_err(database)?;
    account_exists(&tx, user, &input.account_id)?;
    let mut added = 0;
    let mut existing = 0;
    for line in lines {
        let prior:Option<(String,i64,String)>=tx.query_row("SELECT date,amount,description FROM reconciliation_lines WHERE user_id=?1 AND account_id=?2 AND line_id=?3",
            params![user,input.account_id,line.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(database)?;
        if let Some((date, amount, description)) = prior {
            if date != line.date || amount != line.amount || description != line.description {
                return Err(conflict(format!(
                    "La referencia {} ya existe con otros datos.",
                    line.reference.as_deref().unwrap_or(&line.id)
                )));
            }
            existing += 1;
        } else {
            tx.execute("INSERT INTO reconciliation_lines(user_id,account_id,line_id,date,amount,description,reference) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![user,input.account_id,line.id,line.date,line.amount,line.description,line.reference]).map_err(database)?;
            added += 1;
        }
    }
    if input.confirm {
        tx.commit().map_err(database)?;
    }
    Ok(Imported {
        added,
        existing,
        committed: input.confirm,
    })
}
#[derive(Deserialize)]
struct List {
    account_id: String,
    page: Option<u32>,
}
#[derive(Deserialize)]
struct CandidateQuery {
    account_id: String,
    line_id: String,
    page: Option<u32>,
}
#[derive(Serialize)]
struct StatementLine {
    line_id: String,
    date: String,
    amount: i64,
    description: String,
    reference: Option<String>,
    operation_id: Option<String>,
    stale: bool,
}
#[derive(Serialize)]
struct Page<T> {
    rows: Vec<T>,
    page: u32,
    pages: u32,
    total: u32,
}
fn pagination(total: u32, page: Option<u32>) -> (u32, u32, u32) {
    let pages = total.div_ceil(50).max(1);
    let page = page.unwrap_or(1).clamp(1, pages);
    (page, pages, (page - 1) * 50)
}
fn list(c: &Connection, user: &str, q: &List) -> Result<Page<StatementLine>> {
    account_exists(c, user, &q.account_id)?;
    let total = c
        .query_row(
            "SELECT COUNT(*) FROM reconciliation_lines WHERE user_id=?1 AND account_id=?2",
            params![user, q.account_id],
            |r| r.get(0),
        )
        .map_err(database)?;
    let (page, pages, offset) = pagination(total, q.page);
    let mut stmt=c.prepare("SELECT l.line_id,l.date,l.amount,l.description,l.reference,m.operation_id,
        m.operation_id IS NOT NULL AND (o.version IS NOT m.version OR p.delta IS NOT l.amount OR p.date IS NOT l.date)
        FROM reconciliation_lines l LEFT JOIN reconciliations m USING(user_id,account_id,line_id)
        LEFT JOIN finance_operations o ON o.user_id=l.user_id AND o.id=m.operation_id
        LEFT JOIN finance_postings p ON p.user_id=l.user_id AND p.account_id=l.account_id AND p.id=m.operation_id
        WHERE l.user_id=?1 AND l.account_id=?2 ORDER BY l.date DESC,l.line_id LIMIT 50 OFFSET ?3").map_err(database)?;
    let rows = stmt
        .query_map(params![user, q.account_id, offset], |r| {
            Ok(StatementLine {
                line_id: r.get(0)?,
                date: r.get(1)?,
                amount: r.get(2)?,
                description: r.get(3)?,
                reference: r.get(4)?,
                operation_id: r.get(5)?,
                stale: r.get(6)?,
            })
        })
        .map_err(database)?
        .collect::<rusqlite::Result<_>>()
        .map_err(database)?;
    Ok(Page {
        rows,
        total,
        page,
        pages,
    })
}
#[derive(Serialize)]
struct Candidate {
    id: String,
    version: i64,
    description: Option<String>,
    kind: String,
}
const MATCHING: &str=" FROM reconciliation_lines l JOIN finance_postings p ON p.user_id=l.user_id AND p.account_id=l.account_id AND p.date=l.date AND p.delta=l.amount
 JOIN finance_operations o ON o.user_id=p.user_id AND o.id=p.id WHERE l.user_id=?1 AND l.account_id=?2 AND l.line_id=?3
 AND NOT EXISTS(SELECT 1 FROM reconciliations m WHERE m.user_id=l.user_id AND m.account_id=l.account_id AND m.operation_id=o.id AND m.line_id!=l.line_id)";
fn candidates(c: &Connection, user: &str, q: &CandidateQuery) -> Result<Page<Candidate>> {
    let total = c
        .query_row(
            &format!("SELECT COUNT(*){MATCHING}"),
            params![user, q.account_id, q.line_id],
            |r| r.get(0),
        )
        .map_err(database)?;
    let (page, pages, offset) = pagination(total, q.page);
    let mut stmt = c
        .prepare(&format!(
            "SELECT o.id,o.version,o.description,o.kind{MATCHING} ORDER BY o.id LIMIT 50 OFFSET ?4"
        ))
        .map_err(database)?;
    let rows = stmt
        .query_map(params![user, q.account_id, q.line_id, offset], |r| {
            Ok(Candidate {
                id: r.get(0)?,
                version: r.get(1)?,
                description: r.get(2)?,
                kind: r.get(3)?,
            })
        })
        .map_err(database)?
        .collect::<rusqlite::Result<_>>()
        .map_err(database)?;
    Ok(Page {
        rows,
        page,
        pages,
        total,
    })
}
#[derive(Deserialize)]
struct Link {
    account_id: String,
    line_id: String,
    operation_id: String,
    expected_version: i64,
}
fn link(c: &Connection, user: &str, q: &Link) -> Result<()> {
    let did_write=c.execute(&format!("INSERT INTO reconciliations(user_id,account_id,line_id,operation_id,version)
        SELECT l.user_id,l.account_id,l.line_id,o.id,o.version{MATCHING} AND o.id=?4 AND o.version=?5
        ON CONFLICT(user_id,account_id,line_id) DO UPDATE SET version=excluded.version WHERE reconciliations.operation_id=excluded.operation_id"),
        params![user,q.account_id,q.line_id,q.operation_id,q.expected_version]).map_err(database)?;
    if did_write == 0 {
        return Err(conflict(
            "La línea o el movimiento cambiaron. Actualiza las coincidencias.",
        ));
    }
    Ok(())
}
#[derive(Deserialize)]
struct Unlink {
    account_id: String,
    line_id: String,
    operation_id: String,
}
async fn listing(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<List>,
) -> Result<Json<Page<StatementLine>>> {
    tokio::task::spawn_blocking(move || {
        let user = owner(&s, &h)?;
        let c =
            s.db.0
                .lock()
                .map_err(|_| database(rusqlite::Error::InvalidQuery))?;
        list(&c, &user, &q).map(Json)
    })
    .await
    .map_err(|_| database(rusqlite::Error::InvalidQuery))?
}
async fn options(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<CandidateQuery>,
) -> Result<Json<Page<Candidate>>> {
    tokio::task::spawn_blocking(move || {
        let user = owner(&s, &h)?;
        let c =
            s.db.0
                .lock()
                .map_err(|_| database(rusqlite::Error::InvalidQuery))?;
        candidates(&c, &user, &q).map(Json)
    })
    .await
    .map_err(|_| database(rusqlite::Error::InvalidQuery))?
}
async fn upload(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(q): Json<Upload>,
) -> Result<Json<Imported>> {
    tokio::task::spawn_blocking(move || {
        let user = owner(&s, &h)?;
        let mut c =
            s.db.0
                .lock()
                .map_err(|_| database(rusqlite::Error::InvalidQuery))?;
        import(&mut c, &user, &q).map(Json)
    })
    .await
    .map_err(|_| database(rusqlite::Error::InvalidQuery))?
}
async fn attach(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(q): Json<Link>,
) -> Result<StatusCode> {
    tokio::task::spawn_blocking(move || {
        let user = owner(&s, &h)?;
        let c =
            s.db.0
                .lock()
                .map_err(|_| database(rusqlite::Error::InvalidQuery))?;
        link(&c, &user, &q)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
    .map_err(|_| database(rusqlite::Error::InvalidQuery))?
}
async fn detach(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(q): Json<Unlink>,
) -> Result<StatusCode> {
    tokio::task::spawn_blocking(move || {
        let user = owner(&s, &h)?;
        let c =
            s.db.0
                .lock()
                .map_err(|_| database(rusqlite::Error::InvalidQuery))?;
        let n=c.execute("DELETE FROM reconciliations WHERE user_id=?1 AND account_id=?2 AND line_id=?3 AND operation_id=?4",params![user,q.account_id,q.line_id,q.operation_id]).map_err(database)?;
        if n == 0 {
            return Err(conflict("El vínculo cambió. Actualiza el extracto."));
        }
        Ok(StatusCode::NO_CONTENT)
    })
    .await
    .map_err(|_| database(rusqlite::Error::InvalidQuery))?
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/reconciliation", get(listing))
        .route("/api/reconciliation/import", post(upload))
        .route("/api/reconciliation/candidates", get(options))
        .route("/api/reconciliation/match", post(attach))
        .route("/api/reconciliation/unmatch", post(detach))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    #[test]
    fn preview_repeat_and_concurrent_reconciliation_preserve_owner_and_versions() {
        let db = Database::open_memory().unwrap();
        let mut c = db.0.lock().unwrap();
        for user in ["alice", "bob"] {
            c.execute(
                "INSERT INTO finance_accounts VALUES(?1,'cash','Efectivo','cash',500)",
                [user],
            )
            .unwrap();
            c.execute(
                "INSERT INTO finance_categories VALUES(?1,'food','Comida','expense')",
                [user],
            )
            .unwrap();
            c.execute("INSERT INTO finance_operations(user_id,id,version,deleted,kind,account_id,category_id,amount,date) VALUES(?1,'lunch',1,0,'expense','cash','food',45,'2026-11-12')",[user]).unwrap();
        }
        let mut file=Upload{account_id:"cash".into(),csv:"fecha;importe;descripción;referencia\n2026-11-12;-45;Almuerzo;ref1\n2026-11-12;-45;Almuerzo;ref2".into(),confirm:false};
        let listing = List {
            account_id: "cash".into(),
            page: None,
        };
        assert_eq!(import(&mut c, "alice", &file).unwrap().added, 2);
        assert_eq!(list(&c, "alice", &listing).unwrap().total, 0);
        file.confirm = true;
        assert_eq!(import(&mut c, "alice", &file).unwrap().added, 2);
        assert_eq!(import(&mut c, "alice", &file).unwrap().existing, 2);
        assert_eq!(list(&c, "bob", &listing).unwrap().total, 0);
        let other = Upload {
            confirm: true,
            account_id: "cash".into(),
            csv: "fecha,importe,referencia\n2026-11-14,8,nueva\n2026-11-12,-49,ref1".into(),
        };
        assert_eq!(
            import(&mut c, "alice", &other).unwrap_err().0,
            StatusCode::CONFLICT
        );
        assert_eq!(list(&c, "alice", &listing).unwrap().total, 2);
        let mut choice = Link {
            account_id: "cash".into(),
            line_id: "ref:ref1".into(),
            operation_id: "lunch".into(),
            expected_version: 1,
        };
        let q = CandidateQuery {
            account_id: "cash".into(),
            line_id: choice.line_id.clone(),
            page: None,
        };
        assert_eq!(candidates(&c, "alice", &q).unwrap().total, 1);
        assert!(link(&c, "bob", &choice).is_err());
        link(&c, "alice", &choice).unwrap();
        link(&c, "alice", &choice).unwrap();
        assert!(!list(&c, "alice", &listing).unwrap().rows[0].stale);
        choice.line_id = "ref:ref2".into();
        assert_eq!(
            link(&c, "alice", &choice).unwrap_err().0,
            StatusCode::CONFLICT
        );
        c.execute("UPDATE finance_operations SET version=2,amount=50 WHERE user_id='alice' AND id='lunch'",[]).unwrap();
        assert!(list(&c, "alice", &listing).unwrap().rows[0].stale);
        assert_eq!(candidates(&c, "alice", &q).unwrap().total, 0);
    }

    #[test]
    fn dates_and_duplicate_unreferenced_lines_require_explicit_correction() {
        assert!(parse_csv("fecha,importe\n2026-02-29,50").is_err());
        assert!(parse_csv("fecha,importe\n2026-02-28,0").is_err());
        assert!(parse_csv("fecha,importe\n2026-02-28,50\n2026-02-28,50").is_err());
        let input = parse_csv("\u{feff}fecha,importe,descripcion\n2026-02-28,50,\"a,b\"").unwrap();
        assert_eq!(input[0].description, "a,b");
    }
}
