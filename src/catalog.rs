use crate::{
    AppState,
    finance::{self, Account, Category, Error},
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Detail {
    pub entity: String,
    pub id: String,
    pub archived: bool,
    pub institution: String,
    pub notes: String,
    pub cutoff_date: Option<String>,
    pub closing_day: Option<u8>,
    pub due_day: Option<u8>,
}

pub(crate) fn list_in(
    conn: &Connection,
    user: &str,
    since: Option<i64>,
) -> rusqlite::Result<Vec<Detail>> {
    let mut query = conn.prepare_cached("SELECT entity,id,archived,institution,notes,cutoff_date,closing_day,due_day
        FROM catalog_details d WHERE user_id=?1 AND (?2 IS NULL OR EXISTS (
          SELECT 1 FROM finance_changes c WHERE c.user_id=d.user_id AND c.entity=d.entity AND c.id=d.id AND c.seq>?2
        )) ORDER BY entity,id")?;
    query
        .query_map(params![user, since], |row| {
            Ok(Detail {
                entity: row.get(0)?,
                id: row.get(1)?,
                archived: row.get(2)?,
                institution: row.get(3)?,
                notes: row.get(4)?,
                cutoff_date: row.get(5)?,
                closing_day: row.get(6)?,
                due_day: row.get(7)?,
            })
        })?
        .collect()
}

pub(crate) fn is_archived(
    conn: &Connection,
    user: &str,
    entity: &str,
    id: &str,
) -> rusqlite::Result<bool> {
    conn.prepare_cached("SELECT EXISTS(SELECT 1 FROM catalog_details WHERE user_id=? AND entity=? AND id=? AND archived=1)")?
        .query_row(params![user, entity, id], |row| row.get(0))
}

pub(crate) fn cutoff(conn: &Connection, user: &str, id: &str) -> rusqlite::Result<Option<String>> {
    Ok(conn
        .prepare_cached(
            "SELECT cutoff_date FROM catalog_details WHERE user_id=? AND entity='account' AND id=?",
        )?
        .query_row(params![user, id], |row| row.get(0))
        .optional()?
        .flatten())
}

pub(crate) fn save(conn: &Connection, user: &str, detail: &Detail) -> Result<(), Error> {
    let table = match detail.entity.as_str() {
        "account" => "finance_accounts",
        "category" => "finance_categories",
        _ => return Err(Error::Invalid),
    };
    let kind: String = conn
        .query_row(
            &format!("SELECT kind FROM {table} WHERE user_id=? AND id=?"),
            params![user, detail.id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(Error::Invalid)?;
    if detail.institution.len() > 160
        || detail.notes.len() > 4000
        || detail.institution.chars().any(char::is_control)
        || detail.notes.chars().any(|c| c.is_control() && c != '\n')
        || detail
            .cutoff_date
            .as_deref()
            .is_some_and(|date| detail.entity != "account" || !finance::valid_date(date))
        || [detail.closing_day, detail.due_day]
            .into_iter()
            .flatten()
            .any(|day| kind != "card" || !(1..=31).contains(&day))
    {
        return Err(Error::Invalid);
    }
    conn.prepare_cached("INSERT INTO catalog_details VALUES(?,?,?,?,?,?,?,?,?)
        ON CONFLICT(user_id,entity,id) DO UPDATE SET archived=excluded.archived,institution=excluded.institution,
        notes=excluded.notes,cutoff_date=excluded.cutoff_date,closing_day=excluded.closing_day,due_day=excluded.due_day")?
        .execute(params![user,detail.entity,detail.id,detail.archived,detail.institution,detail.notes,detail.cutoff_date,detail.closing_day,detail.due_day])?;
    if detail.entity == "account" {
        finance::validate_balance(conn, user, &detail.id)?;
    }
    finance::changed(conn, user, &detail.entity, &detail.id)
}

#[derive(Deserialize)]
struct Archive {
    entity: String,
    id: String,
    archived: bool,
}

async fn archive(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Archive>,
) -> Result<StatusCode, StatusCode> {
    let user = finance::bound_user(&state.db, &headers)?;
    finance::write(&state.db, |conn| {
        let mut detail = conn
            .prepare_cached("SELECT entity,id,archived,institution,notes,cutoff_date,closing_day,due_day FROM catalog_details WHERE user_id=?1 AND entity=?2 AND id=?3")?
            .query_row(params![user.id, input.entity, input.id], |row| {
                Ok(Detail {
                    entity: row.get(0)?,
                    id: row.get(1)?,
                    archived: row.get(2)?,
                    institution: row.get(3)?,
                    notes: row.get(4)?,
                    cutoff_date: row.get(5)?,
                    closing_day: row.get(6)?,
                    due_day: row.get(7)?,
                })
            })
            .optional()?
            .unwrap_or(Detail {
                entity: input.entity,
                id: input.id,
                ..Default::default()
            });
        detail.archived = input.archived;
        save(conn, &user.id, &detail)
    })
    .map_err(finance::status)?;
    Ok(StatusCode::NO_CONTENT)
}

const SUGGESTIONS: &[(&str, &str, &str)] = &[
    ("food", "Alimentación", "expense"),
    ("transport", "Transporte", "expense"),
    ("housing", "Vivienda", "expense"),
    ("health", "Salud", "expense"),
    ("services", "Servicios", "expense"),
    ("leisure", "Ocio", "expense"),
    ("salary", "Salario", "income"),
    ("other_income", "Otros ingresos", "income"),
];

#[derive(Deserialize)]
struct Suggested {
    selected: Vec<String>,
}

async fn suggest(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Suggested>,
) -> Result<StatusCode, StatusCode> {
    let user = finance::bound_user(&state.db, &headers)?;
    if input.selected.is_empty() || input.selected.len() > SUGGESTIONS.len() {
        return Err(StatusCode::BAD_REQUEST);
    }
    finance::write(&state.db, |conn| {
        for key in input.selected {
            let (_, name, kind) = SUGGESTIONS
                .iter()
                .find(|entry| entry.0 == key)
                .ok_or(Error::Invalid)?;
            let id = format!("suggested:{key}");
            let existing: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM finance_categories
                WHERE user_id=? AND (id=? OR (kind=? AND name=? COLLATE NOCASE)))",
                params![user.id, id, kind, name],
                |row| row.get(0),
            )?;
            if !existing {
                finance::save_category_in(
                    conn,
                    &user.id,
                    &Category {
                        id,
                        name: (*name).into(),
                        kind: (*kind).into(),
                    },
                )?;
            }
        }
        Ok(())
    })
    .map_err(finance::status)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct AccountInput {
    #[serde(flatten)]
    pub account: Account,
    pub details: Option<Detail>,
}

pub(crate) fn save_account(
    conn: &Connection,
    user: &str,
    input: &mut AccountInput,
) -> Result<(), Error> {
    finance::upsert_account_in(conn, user, &input.account)?;
    match &mut input.details {
        Some(detail) => {
            detail.id = input.account.id.clone();
            detail.entity = "account".into();
            detail.archived = is_archived(conn, user, "account", &detail.id)?;
            save(conn, user, detail)
        }
        None => {
            finance::validate_balance(conn, user, &input.account.id)?;
            finance::changed(conn, user, "account", &input.account.id)
        }
    }
}

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/catalog/archive", post(archive))
        .route("/api/catalog/suggest", post(suggest))
}
