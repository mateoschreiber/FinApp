//! Paginated movements for the authenticated user's ledger.
use crate::{AppState, finance};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

#[derive(Default, Deserialize)]
struct Filters {
    from: Option<String>,
    to: Option<String>,
    text: Option<String>,
    account: Option<String>,
    category: Option<String>,
    kind: Option<String>,
    page: Option<u32>,
    page_size: Option<u16>,
}

#[derive(Serialize)]
struct Page {
    rows: Vec<finance::Operation>,
    total: u64,
    page: u64,
    pages: u64,
    page_size: u16,
}

const FILTER: &str = " FROM finance_operations WHERE user_id=?1 AND deleted=0
    AND (?2 IS NULL OR date>=?2) AND (?3 IS NULL OR date<=?3)
    AND (?4 IS NULL OR account_id=?4 OR destination_account_id=?4 OR card_account_id=?4)
    AND (?5 IS NULL OR category_id=?5) AND (?6 IS NULL OR kind=?6)
    AND (?7 IS NULL OR instr(lower(coalesce(description,'')),lower(?7))>0
                    OR instr(lower(coalesce(notes,'')),lower(?7))>0)";

fn nonempty(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn load(conn: &Connection, user: &str, filters: &Filters) -> Result<Page, finance::Error> {
    let from = nonempty(&filters.from);
    let to = nonempty(&filters.to);
    let search = nonempty(&filters.text);
    let account = nonempty(&filters.account);
    let category = nonempty(&filters.category);
    let kind = nonempty(&filters.kind);
    let size = filters.page_size.unwrap_or(25);
    let requested = filters.page.unwrap_or(1);
    if from.into_iter().chain(to).any(|s| !finance::valid_date(s))
        || from.zip(to).is_some_and(|(start, end)| start > end)
        || search.is_some_and(|s| s.len() > 500)
        || account.into_iter().chain(category).any(|id| id.len() > 128)
        || !(1..=100).contains(&size)
        || requested == 0
    {
        return Err(finance::Error::Invalid);
    }
    let total: u64 = conn.query_row(
        &format!("SELECT COUNT(*){FILTER}"),
        params![user, from, to, account, category, kind, search],
        |row| row.get(0),
    )?;
    let pages = total.div_ceil(u64::from(size)).max(1);
    let page = u64::from(requested).min(pages);
    let offset = (page - 1) * u64::from(size);
    let mut query = conn.prepare(&format!(
        "SELECT data{FILTER} ORDER BY date DESC,id LIMIT ?8 OFFSET ?9"
    ))?;
    let mut rows = Vec::new();
    for result in query.query_map(
        params![
            user, from, to, account, category, kind, search, size, offset
        ],
        |row| row.get::<_, String>(0),
    )? {
        rows.push(serde_json::from_str(&result?).map_err(|_| finance::Error::Database)?);
    }
    Ok(Page {
        rows,
        total,
        page,
        pages,
        page_size: size,
    })
}

async fn history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(filters): Query<Filters>,
) -> Result<Json<Page>, StatusCode> {
    let user = finance::bound_user(&state.db, &headers)?;
    let conn = state
        .db
        .0
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    load(&conn, &user.id, &filters)
        .map(Json)
        .map_err(|e| match e {
            finance::Error::Invalid => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/history", get(history))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    #[test]
    fn literal_search_and_destination_filter_paginate_only_live_owned_records() {
        let db = Database::open_memory().unwrap();
        let conn = db.0.lock().unwrap();
        for user in ["alice", "bob"] {
            for id in ["a", "b"] {
                conn.execute(
                    "INSERT INTO finance_accounts VALUES(?1,?2,?2,'bank',0)",
                    params![user, id],
                )
                .unwrap();
            }
            for (id, deleted) in [("x", false), ("y", false), ("z", true)] {
                conn.execute("INSERT INTO finance_operations(user_id,id,version,deleted,kind,account_id,destination_account_id,amount,date,description)
                    VALUES(?1,?2,1,?3,'transfer','a','b',5,'2026-01-02','50%')",params![user,id,deleted]).unwrap();
            }
        }
        let query = Filters {
            account: Some("b".into()),
            text: Some("%".into()),
            page: Some(2),
            page_size: Some(1),
            ..Default::default()
        };
        let page = load(&conn, "alice", &query).unwrap();
        assert_eq!((page.total, page.page, page.pages), (2, 2, 2));
        assert_eq!(page.rows[0].id, "y");
        assert!(
            load(
                &conn,
                "alice",
                &Filters {
                    from: Some("2026-02-29".into()),
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
}
