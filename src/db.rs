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

    fn initialize(conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch(include_str!("../migrations/001_auth.sql"))?;
        conn.execute_batch(crate::finance::MIGRATION)?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    pub fn is_initialized(&self) -> rusqlite::Result<bool> {
        let conn = self.0.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.query_row("SELECT EXISTS(SELECT 1 FROM users)", [], |row| row.get(0))
    }
}
