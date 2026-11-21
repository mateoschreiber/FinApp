//! F9.2A: migration 009 must preserve V1 loans created under the old schema.
use finapp::db::Database;
use rusqlite::Connection;
use std::time::{SystemTime, UNIX_EPOCH};

const OLD_MIGRATIONS: &[&str] = &[
    include_str!("../migrations/001_auth.sql"),
    include_str!("../migrations/002_finance.sql"),
    include_str!("../migrations/003_ledger.sql"),
    include_str!("../migrations/004_catalog.sql"),
    include_str!("../migrations/005_planning.sql"),
    include_str!("../migrations/006_credit.sql"),
    include_str!("../migrations/007_operation_history.sql"),
    include_str!("../migrations/008_reconciliation.sql"),
];

fn old_database(path: &std::path::Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("CREATE TABLE IF NOT EXISTS schema_versions(version INTEGER PRIMARY KEY)")
        .unwrap();
    for sql in OLD_MIGRATIONS {
        conn.execute_batch(sql).unwrap();
    }
    for version in 1..=8 {
        conn.execute("INSERT INTO schema_versions(version) VALUES(?1)", [version])
            .unwrap();
    }
    conn.execute("INSERT INTO users VALUES('u','u','-',0)", [])
        .unwrap();
    conn.execute(
        "INSERT INTO finance_accounts VALUES('u','bank','Banco','bank',0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO finance_operations(user_id,id,version,deleted,kind,account_id,amount,date,description)
         VALUES('u','draw',1,0,'loan_draw','bank',1000,'2025-01-02','desembolso histórico')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO credit_loans VALUES('u','old','Antiguo','bank',1000,'2025-01-02','2025-02-02',2,'german',1200,'nominal')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO credit_repayments VALUES('u','oldpay','old','2025-02-01',200,20,1)",
        [],
    )
    .unwrap();
}

#[test]
fn migration_009_preserves_legacy_loans_and_is_idempotent() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("finapp-009-{unique}.sqlite"));
    old_database(&path);

    let db = Database::open(&path).unwrap();
    {
        let conn = db.0.lock().unwrap();
        let applied: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_versions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied, 9);
        let version: u8 = conn
            .query_row(
                "SELECT schedule_version FROM credit_loans WHERE id='old'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, 1, "historical loans stay on V1");
        let loan: (String, i64, String, String, u16, String, u32, String) = conn
            .query_row(
                "SELECT name,principal,draw_date,first_due,periods,method,rate_bps,rate_type
                 FROM credit_loans WHERE id='old'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            loan,
            (
                "Antiguo".into(),
                1000,
                "2025-01-02".into(),
                "2025-02-02".into(),
                2,
                "german".into(),
                1200,
                "nominal".into()
            )
        );
        let repayment: (i64, i64, i64) = conn
            .query_row(
                "SELECT principal,interest,fee FROM credit_repayments WHERE id='oldpay'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(repayment, (200, 20, 1));
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
    drop(db);

    // A second open must not reapply the migration or duplicate schema versions.
    let db = Database::open(&path).unwrap();
    let applied: i64 =
        db.0.lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM schema_versions", [], |r| r.get(0))
            .unwrap();
    assert_eq!(applied, 9);
    drop(db);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn new_loans_get_v2_by_default_after_migration() {
    let db = Database::open_memory().unwrap();
    db.0.lock()
        .unwrap()
        .execute("INSERT INTO users VALUES('u','u','-',0)", [])
        .unwrap();
    db.0.lock()
        .unwrap()
        .execute(
            "INSERT INTO finance_accounts VALUES('u','bank','Banco','bank',0)",
            [],
        )
        .unwrap();
    // Insert with the explicit V2 value used by mutate(); the column default must
    // still be 1 for any caller that omits it (legacy behaviour).
    db.0.lock()
        .unwrap()
        .execute(
            "INSERT INTO credit_loans(user_id,id,name,account_id,principal,draw_date,first_due,periods,method,rate_bps,rate_type,schedule_version)
             VALUES('u','new','Nuevo','bank',1000,'2025-01-02','2025-02-02',2,'french',1200,'nominal',2)",
            [],
        )
        .unwrap();
    let conn = db.0.lock().unwrap();
    let version: u8 = conn
        .query_row(
            "SELECT schedule_version FROM credit_loans WHERE id='new'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(version, 2);
    let invalid = conn.execute(
        "INSERT INTO credit_loans(user_id,id,name,account_id,principal,draw_date,first_due,periods,method,rate_bps,rate_type,schedule_version)
         VALUES('u','bad','Mal','bank',1000,'2025-01-02','2025-02-02',2,'french',1200,'nominal',3)",
        [],
    );
    assert!(invalid.is_err(), "only versions 1 and 2 are allowed");
}
