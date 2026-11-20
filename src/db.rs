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
        ];
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let current: i64 = tx.query_row("SELECT COALESCE(MAX(version),0) FROM schema_versions", [], |r| r.get(0))?;
        if current > MIGRATIONS.len() as i64 { return Err(rusqlite::Error::InvalidQuery); }
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            tx.execute_batch(sql)?;
            tx.execute("INSERT INTO schema_versions(version) VALUES(?1)", [index as i64 + 1])?;
        }
        tx.commit()?;
        // Restored backups can repeat a previous cursor. A new connection generation forces one fresh snapshot.
        conn.execute_batch("CREATE TEMP TABLE sync_epoch AS SELECT lower(hex(randomblob(16))) AS id")?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    pub fn is_initialized(&self) -> rusqlite::Result<bool> {
        let conn = self.0.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.query_row("SELECT EXISTS(SELECT 1 FROM users)", [], |row| row.get(0))
    }
}
