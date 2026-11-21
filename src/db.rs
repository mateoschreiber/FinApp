use rusqlite::Connection;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

/// Database access is serialized so related writes can use a SQLite transaction.
#[derive(Clone)]
pub struct Database(pub Arc<Mutex<Connection>>);

impl Database {
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        Self::initialize(Connection::open(path)?)
    }

    pub fn open_memory() -> rusqlite::Result<Self> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(mut conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE IF NOT EXISTS schema_versions(version INTEGER PRIMARY KEY)")?;
        const MIGRATIONS: &[&str] = &[
            include_str!("../migrations/001_auth.sql"),
            include_str!("../migrations/002_finance.sql"),
            include_str!("../migrations/003_ledger.sql"),
            include_str!("../migrations/004_catalog.sql"),
            include_str!("../migrations/005_planning.sql"),
            include_str!("../migrations/006_credit.sql"),
            include_str!("../migrations/007_operation_history.sql"),
            include_str!("../migrations/008_reconciliation.sql"),
            include_str!("../migrations/009_schedule_version.sql"),
        ];
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let current: i64 = tx.query_row(
            "SELECT COALESCE(MAX(version),0) FROM schema_versions",
            [],
            |r| r.get(0),
        )?;
        if current > MIGRATIONS.len() as i64 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_versions(version) VALUES(?1)",
                [index as i64 + 1],
            )?;
        }
        tx.commit()?;
        // Restored backups can repeat a previous cursor. A new connection generation forces one fresh snapshot.
        conn.execute_batch(
            "CREATE TEMP TABLE sync_epoch AS SELECT lower(hex(randomblob(16))) AS id",
        )?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    pub fn is_initialized(&self) -> rusqlite::Result<bool> {
        let conn = self.0.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.query_row("SELECT EXISTS(SELECT 1 FROM users)", [], |row| row.get(0))
    }
}

/// Logs only the stage, the place and the SQLite code. The SQLite message can include
/// table or column text and values, so it is never printed.
pub(crate) fn log_sqlite(stage: &str, place: &str, error: &rusqlite::Error) {
    eprintln!("{}", sqlite_diagnostic(stage, place, error));
}

fn sqlite_diagnostic(stage: &str, place: &str, error: &rusqlite::Error) -> String {
    match error {
        rusqlite::Error::SqliteFailure(e, _) => format!(
            "finapp sqlite stage={stage} place={place} code={:?} extended={}",
            e.code, e.extended_code
        ),
        _ => format!("finapp sqlite stage={stage} place={place} error=non_sqlite"),
    }
}

#[cfg(test)]
mod tests {
    use super::{Database, sqlite_diagnostic};

    #[test]
    fn effective_busy_timeout_is_five_seconds() {
        let db = Database::open_memory().unwrap();
        let conn = db.0.lock().unwrap();
        let ms: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(ms, 5000);
    }

    #[test]
    fn diagnostic_names_the_sqlite_code_without_the_message() {
        let error = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(5),
            Some("secreto de cuenta Carga 1500000".into()),
        );
        let line = sqlite_diagnostic("commit", "importación", &error);
        assert!(line.contains("stage=commit"), "{line}");
        assert!(line.contains("code=DatabaseBusy"), "{line}");
        assert!(line.contains("extended=5"), "{line}");
        assert!(
            !line.contains("secreto") && !line.contains("1500000"),
            "{line}"
        );
    }

    #[test]
    fn diagnostic_marks_non_sqlite_errors() {
        let line = sqlite_diagnostic(
            "source_lookup",
            "movimientos fila 7",
            &rusqlite::Error::QueryReturnedNoRows,
        );
        assert_eq!(
            line,
            "finapp sqlite stage=source_lookup place=movimientos fila 7 error=non_sqlite"
        );
    }
}
