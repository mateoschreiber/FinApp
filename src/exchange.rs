//! Import previews and confirmations validate the same rows and run against a transaction.
use crate::{
    AppState, catalog,
    db::log_sqlite,
    finance::{self, Account, Category, Change, Outcome},
    planning,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use calamine::{Data, DataRef, DataType, Reader, Xlsx};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
};

const MIB: usize = 1024 * 1024;
const MAX_COMPRESSED: usize = 5 * MIB;
const MAX_EXPANDED: usize = 32 * MIB;
const MAX_MOVEMENTS: u32 = 25000;
const MAX_CATALOG_ROWS: u32 = 2000;
const MAX_COLUMNS: u32 = 128;

type Row = BTreeMap<String, String>;
type Problem = (StatusCode, String);
type Rows = Vec<(u32, Row)>;
type Sheets = Vec<(&'static str, Rows)>;

#[derive(Deserialize)]
struct Upload {
    file: String,
    #[serde(default)]
    mapping: BTreeMap<String, String>,
    /// External category id -> internal category id. Never applied to accounts or operations.
    #[serde(default)]
    category_mapping: BTreeMap<String, String>,
    /// External operation id -> internal operation id, for related-movement references.
    #[serde(default)]
    operation_mapping: BTreeMap<String, String>,
    #[serde(default)]
    include_before_cutoff: bool,
    preview_token: Option<String>,
    /// Explicit acceptance of saldo_actual differences; must repeat the preview token.
    #[serde(default)]
    ack_saldo: Option<String>,
}

#[derive(Default, Serialize)]
struct Preview {
    accounts: Vec<SourceAccount>,
    categories: Vec<SourceCategory>,
    references: Vec<SourceReference>,
    added: usize,
    duplicates: usize,
    skipped: usize,
    errors: Vec<String>,
    warnings: Vec<String>,
    /// Structured saldo_actual differences; the warnings above stay for compatibility.
    differences: Vec<BalanceDiscrepancy>,
    balances: BTreeMap<String, String>,
    token: String,
    committed: bool,
}

#[derive(Serialize)]
struct BalanceDiscrepancy {
    account_id: String,
    declared_balance: i64,
    calculated_balance: i64,
    difference: i64,
}
#[derive(Serialize)]
struct SourceAccount {
    id: String,
    name: String,
    kind: String,
    linked: Option<String>,
}
#[derive(Serialize)]
struct SourceCategory {
    id: String,
    name: String,
    kind: String,
    linked: Option<String>,
}
#[derive(Serialize)]
struct SourceReference {
    source_id: String,
    movement_id: String,
}

fn key(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .replace('á', "a")
        .replace('é', "e")
        .replace('í', "i")
        .replace('ó', "o")
        .replace('ú', "u")
        .replace([' ', '-'], "_")
}
fn value<'a>(row: &'a Row, field: &str) -> &'a str {
    row.get(field).map(String::as_str).unwrap_or("")
}
fn optional(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
}
fn kind(s: &str) -> &str {
    match s {
        "efectivo" => "cash",
        "banco" | "cuenta_bancaria" | "bancaria" => "bank",
        "billetera" | "billetera_digital" => "wallet",
        "tarjeta" | "tarjeta_de_credito" | "tarjeta_credito" | "tc" => "card",
        "ingreso" => "income",
        "gasto" | "egreso" | "compra_tc" | "compra_tarjeta" | "compra_financiada" => "expense",
        "pago_tarjeta" | "pago_de_tarjeta" | "pago_tc" => "payment",
        "transferencia" => "transfer",
        "devolucion" | "reintegro" => "refund",
        "desembolso" | "desembolso_prestamo" | "alta_prestamo" => "loan_draw",
        "pago_prestamo" | "pago_de_prestamo" | "pago_deuda" => "loan_repayment",
        v => v,
    }
}
fn fingerprint(row: &Row) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(row).expect("text-only row"),
    ))
}

fn book(bytes: &[u8]) -> Result<Sheets, String> {
    let invalid = || "El archivo debe ser un libro XLSX válido.".to_string();
    if bytes.len() > MAX_COMPRESSED {
        return Err("El XLSX excede 5 MiB.".into());
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| invalid())?;
    if zip.len() > 512
        || zip
            .decompressed_size()
            .is_none_or(|size| size > MAX_EXPANDED as u128)
    {
        return Err("El libro excede el tamaño admitido.".into());
    }
    let mut expanded = 0;
    for index in 0..zip.len() {
        let mut file = zip.by_index(index).map_err(|_| invalid())?;
        let limit = MAX_EXPANDED - expanded + 1;
        expanded += std::io::copy(&mut file.by_ref().take(limit as u64), &mut std::io::sink())
            .map_err(|_| invalid())? as usize;
        if expanded > MAX_EXPANDED {
            return Err("El libro excede el tamaño admitido.".into());
        }
    }
    let mut workbook: Xlsx<_> = Xlsx::new(Cursor::new(bytes)).map_err(|_| invalid())?;
    let mut result = Vec::new();
    for (sheet, required, max_rows) in [
        (
            "cuentas",
            &["cuenta_id", "tipo", "nombre"][..],
            MAX_CATALOG_ROWS,
        ),
        (
            "categorias",
            &["categoria_id", "tipo", "nombre"][..],
            MAX_CATALOG_ROWS,
        ),
        (
            "movimientos",
            &["movimiento_id", "tipo", "fecha", "monto"][..],
            MAX_MOVEMENTS,
        ),
    ] {
        let name = workbook
            .sheet_names()
            .iter()
            .find(|n| key(n) == sheet)
            .cloned()
            .ok_or_else(|| format!("Falta la hoja {sheet}."))?;
        let mut reader = workbook
            .worksheet_cells_reader(&name)
            .map_err(|_| format!("No se puede leer {name}."))?;
        let mut header_row = None;
        let mut headers = BTreeMap::new();
        let mut rows: BTreeMap<u32, Row> = BTreeMap::new();
        while let Some(cell) = reader
            .next_cell()
            .map_err(|_| format!("Celda inválida en {name}."))?
        {
            let (row, column) = cell.get_position();
            let value = cell.get_value();
            if matches!(value, DataRef::Empty) {
                continue;
            }
            let first = *header_row.get_or_insert(row);
            if row > first + max_rows || column >= MAX_COLUMNS {
                return Err(format!(
                    "La hoja {name} excede el límite de filas o columnas."
                ));
            }
            if row == first {
                let label = key(&Data::from(value.clone()).to_string());
                if !label.is_empty() && headers.values().any(|v| v == &label) {
                    return Err(format!("Encabezado repetido en {name}: {label}."));
                }
                headers.insert(column, label);
            } else if let Some(label) = headers.get(&column).filter(|h| !h.is_empty()) {
                let text = match value {
                    DataRef::DateTime(_) => value
                        .as_date()
                        .map(|date| date.to_string())
                        .ok_or_else(|| format!("Fecha inválida en {name}, fila {}.", row + 1))?,
                    _ => Data::from(value.clone()).to_string().trim().to_owned(),
                };
                rows.entry(row + 1).or_default().insert(label.clone(), text);
            }
        }
        for column in required {
            if !headers.values().any(|value| value == column) {
                return Err(format!("Falta la columna {column} en {name}."));
            }
        }
        result.push((sheet, rows.into_iter().collect()));
    }
    Ok(result)
}

fn number(s: &str) -> Result<i64, String> {
    s.parse::<i64>()
        .ok()
        .filter(|value| value.unsigned_abs() <= finance::SAFE as u64)
        .ok_or_else(|| format!("Importe o saldo inválido «{s}». Usa guaraníes enteros."))
}
fn opening_amount(s: &str) -> Result<i64, String> {
    if s.is_empty() { Ok(0) } else { number(s) }
}

fn parse_date(s: &str) -> Result<String, String> {
    let date = if let Some((day, rest)) = s.split_once('/') {
        let (month, year) = rest
            .split_once('/')
            .ok_or_else(|| format!("Fecha inválida «{s}». Usa dd/mm/aaaa o aaaa-mm-dd."))?;
        // Two-digit years are ambiguous; never pad them into a wrong century.
        if year.len() != 4 {
            return Err(format!(
                "Fecha inválida «{s}». El año necesita cuatro dígitos."
            ));
        }
        format!("{year}-{month:0>2}-{day:0>2}")
    } else {
        s.split('T').next().unwrap_or(s).to_owned()
    };
    if finance::valid_date(&date) {
        Ok(date)
    } else {
        Err(format!("Fecha inválida «{s}». Usa aaaa-mm-dd."))
    }
}
fn archived(row: &Row) -> bool {
    matches!(
        key(value(row, "activa")).as_str(),
        "false" | "no" | "0" | "inactiva"
    )
}
fn finance_error(error: finance::Error) -> String {
    match error {
        finance::Error::Invalid => {
            "Datos inválidos o referencias incompatibles. Revisa la cuenta, categoría y tipo."
        }
        finance::Error::DuplicateRequest => "Esta solicitud ya existe con otros datos.",
        finance::Error::Database => "No se pudo guardar esta fila.",
    }
    .into()
}

struct Import<'a> {
    user: &'a str,
    input: &'a Upload,
    mapping: BTreeMap<String, String>,
    expected: BTreeMap<String, i64>,
    report: Preview,
    sheet: &'static str,
    line: u32,
}

impl Import<'_> {
    fn place(&self) -> String {
        format!("{} fila {}", self.sheet, self.line)
    }

    /// Resolves an external category id to the internal id the row must use.
    /// `expected_kind` is the category kind required by the importing row.
    fn category_target(
        &self,
        conn: &Connection,
        external: &str,
        expected_kind: &str,
    ) -> Result<String, String> {
        if let Some(mapped) = self.input.category_mapping.get(external) {
            let (kind, exists): (String, bool) = conn
                .query_row(
                    "SELECT kind,1 FROM finance_categories WHERE user_id=?1 AND id=?2",
                    params![self.user, mapped],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? == 1)),
                )
                .optional()
                .map_err(|_| "No se pudo consultar la categoría vinculada.")?
                .unwrap_or_else(|| (String::new(), false));
            if !exists {
                return Err("La categoría vinculada no existe.".into());
            }
            if kind != expected_kind {
                return Err("La categoría vinculada tiene un tipo diferente.".into());
            }
            return Ok(mapped.clone());
        }
        Ok(external.to_owned())
    }

    /// Resolves an external related-operation id to the internal operation id.
    /// Persisted import_sources win over a new explicit mapping.
    fn operation_target(&self, conn: &Connection, external: &str) -> Result<String, String> {
        let prior: Option<String> = conn
            .query_row(
                "SELECT target_id FROM import_sources WHERE user_id=?1 AND entity='operation' AND source_id=?2",
                params![self.user, external],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "No se pudo consultar la referencia del movimiento.")?;
        if let Some(target) = prior {
            if self
                .input
                .operation_mapping
                .get(external)
                .is_some_and(|value| value != &target)
            {
                return Err("El movimiento ya está vinculado a otra operación de FinApp.".into());
            }
            return Ok(target);
        }
        if let Some(mapped) = self.input.operation_mapping.get(external) {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM finance_operations WHERE user_id=?1 AND id=?2)",
                    params![self.user, mapped],
                    |r| r.get(0),
                )
                .map_err(|_| "No se pudo comprobar la operación vinculada.")?;
            if !exists {
                return Err("La operación vinculada no existe.".into());
            }
            // Persist the logical link so later files can resolve the same external
            // reference without repeating the explicit mapping.
            let fingerprint = hex::encode(Sha256::digest(
                serde_json::to_vec(&("operation_mapping", external, mapped))
                    .expect("serializable mapping"),
            ));
            conn.execute(
                "INSERT INTO import_sources VALUES(?,?,?,?,?)",
                params![self.user, "operation", external, mapped, fingerprint],
            )
            .map_err(|_| "No se pudo guardar la referencia del movimiento.")?;
            return Ok(mapped.clone());
        }
        let direct: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM finance_operations WHERE user_id=?1 AND id=?2)",
                params![self.user, external],
                |r| r.get(0),
            )
            .map_err(|_| "No se pudo comprobar la referencia del movimiento.")?;
        if direct {
            return Ok(external.to_owned());
        }
        Err("No se pudo resolver la referencia del movimiento.".into())
    }
}

impl Import<'_> {
    fn add_account(
        &mut self,
        conn: &Connection,
        row: &Row,
        id: &str,
        target: &str,
        ty: &str,
    ) -> Result<(), String> {
        let balance = opening_amount(value(row, "saldo_inicial"))?;
        if self.mapping.contains_key(id) {
            let (actual, initial): (String, i64) = conn
                .query_row(
                    "SELECT kind,initial_balance FROM finance_accounts WHERE user_id=? AND id=?",
                    params![self.user, target],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(|_| "La cuenta vinculada no existe.")?;
            if actual != ty {
                return Err("La cuenta vinculada tiene un tipo diferente.".into());
            }
            if initial != balance {
                self.report.warnings.push(format!("Cuenta {id}: saldo inicial en FinApp {initial}, saldo inicial del XLSX {balance}."));
            }
        } else {
            finance::save_account_in(
                conn,
                self.user,
                &Account {
                    id: target.into(),
                    name: value(row, "nombre").into(),
                    kind: ty.into(),
                    initial_balance: balance,
                },
            )
            .map_err(finance_error)?;
            let day = |field| {
                optional(value(row, field))
                    .map(|s| s.parse::<u8>().map_err(|_| "Día de tarjeta inválido."))
                    .transpose()
            };
            catalog::save(
                conn,
                self.user,
                &catalog::Detail {
                    entity: "account".into(),
                    id: target.into(),
                    archived: archived(row),
                    institution: value(row, "entidad").into(),
                    notes: value(row, "notas").into(),
                    cutoff_date: optional(value(row, "fecha_corte"))
                        .as_deref()
                        .map(parse_date)
                        .transpose()?,
                    closing_day: day("dia_cierre")?,
                    due_day: day("dia_vencimiento")?,
                },
            )
            .map_err(finance_error)?;
        }
        if let Some(saldo) = optional(value(row, "saldo_actual")) {
            self.expected.insert(target.into(), number(&saldo)?);
        }
        Ok(())
    }

    fn add_operation(
        &mut self,
        conn: &Connection,
        row: &Row,
        id: &str,
        ty: &str,
    ) -> Result<(), String> {
        let date = parse_date(value(row, "fecha"))?;
        let amount = number(value(row, "monto"))?;
        let mapped = |field| {
            let id = value(row, field);
            self.mapping.get(id).cloned().or_else(|| optional(id))
        };
        let dest = mapped("cuenta_destino_id");
        let origin = mapped("cuenta_origen_id");
        let account = if matches!(ty, "income" | "loan_draw" | "refund") {
            dest.clone().or(origin)
        } else {
            origin
        };
        let mut category = optional(value(row, "categoria_id"));
        if let Some(external) = category.as_deref()
            && matches!(ty, "income" | "expense" | "refund" | "loan_repayment")
        {
            let expected = if ty == "income" { "income" } else { "expense" };
            category = Some(self.category_target(conn, external, expected)?);
        }
        let original_external = optional(value(row, "movimiento_relacionado_id"));
        let original = match original_external.as_deref() {
            Some(external) if ty == "refund" => match self.operation_target(conn, external) {
                Ok(target) => Some(target),
                Err(error) => {
                    self.report.references.push(SourceReference {
                        source_id: external.to_owned(),
                        movement_id: id.to_owned(),
                    });
                    return Err(error);
                }
            },
            Some(external) => Some(external.to_owned()),
            None => None,
        };
        if ty == "refund" && category.is_none() {
            category = conn
                .query_row(
                    "SELECT category_id FROM finance_operations WHERE user_id=? AND id=?",
                    params![self.user, original],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|_| "No se pudo leer el gasto original.")?
                .flatten();
        }
        for acct in BTreeSet::from_iter([account.as_deref(), dest.as_deref()].into_iter().flatten())
        {
            let cutoff = catalog::cutoff(conn, self.user, acct)
                .map_err(|_| "No se pudo consultar la fecha de corte.")?;
            if cutoff.is_some_and(|cutoff| date < cutoff) {
                self.report.warnings.push(format!(
                    "Movimiento {id}: anterior a la fecha de corte de {acct}."
                ));
                if !self.input.include_before_cutoff {
                    return Err("Confirma la importación del histórico anterior al corte.".into());
                }
            }
        }
        let mut change = Change {
            request_id: format!("xlsx-{}", fingerprint(row)),
            id: id.into(),
            expected_version: 0,
            deleted: false,
            kind: Some(ty.into()),
            account_id: account,
            category_id: if matches!(ty, "income" | "expense" | "refund") {
                category.clone()
            } else {
                None
            },
            card_account_id: if ty == "payment" { dest.clone() } else { None },
            destination_account_id: if ty == "transfer" { dest } else { None },
            original_operation_id: if ty == "refund" { original } else { None },
            amount: Some(amount),
            date: Some(date),
            description: optional(value(row, "descripcion")),
            notes: optional(value(row, "notas")),
        };
        let apply = |change: &Change| -> Result<(), String> {
            match finance::apply_import_in(conn, self.user, change).map_err(finance_error)? {
                Outcome::Applied(_) => Ok(()),
                Outcome::Conflict(_) => Err("El ID del movimiento ya existe.".into()),
            }
        };
        if ty == "loan_repayment" {
            let interest = opening_amount(value(row, "interes"))?;
            let fees = opening_amount(value(row, "comisiones"))?;
            let principal = if value(row, "capital").is_empty() {
                amount - interest - fees
            } else {
                number(value(row, "capital"))?
            };
            if [principal, interest, fees].iter().any(|n| *n < 0)
                || principal + interest + fees != amount
                || amount <= 0
            {
                return Err(
                    "El capital, los intereses y las comisiones no suman el importe del pago."
                        .into(),
                );
            }
            let first = if principal > 0 {
                "capital"
            } else if interest > 0 {
                "interes"
            } else {
                "comision"
            };
            for (part, amount) in [
                ("capital", principal),
                ("interes", interest),
                ("comision", fees),
            ] {
                if amount == 0 {
                    continue;
                }
                change.id = if part == first {
                    id.into()
                } else {
                    format!("xlsx-{part}-{}", fingerprint(row))
                };
                change.request_id = format!("xlsx-{part}-{}", fingerprint(row));
                change.amount = Some(amount);
                change.kind = Some(
                    if part == "capital" {
                        "loan_repayment"
                    } else {
                        "expense"
                    }
                    .into(),
                );
                change.category_id = if part == "capital" {
                    None
                } else {
                    category.clone()
                };
                if part != "capital" {
                    change.description = Some(format!("{part} · {}", value(row, "descripcion")));
                }
                apply(&change)?;
            }
        } else {
            apply(&change)?;
        }
        Ok(())
    }

    fn row(
        &mut self,
        conn: &Connection,
        entity: &str,
        row: &Row,
        id: &str,
        ty: &str,
    ) -> Result<(), String> {
        let table = match entity {
            "account" => "finance_accounts",
            "category" => "finance_categories",
            _ => "finance_operations",
        };
        let fingerprint = fingerprint(row);
        let prior=conn.query_row("SELECT target_id,fingerprint FROM import_sources WHERE user_id=? AND entity=? AND source_id=?",
            params![self.user,entity,id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))
            .optional()
            .map_err(|e| {
                log_sqlite("source_lookup", &self.place(), &e);
                "No se pudo consultar el historial de importación."
            })?;
        if let Some((target, hash)) = prior {
            if fingerprint != hash {
                return Err("El ID importado previamente tiene datos diferentes.".into());
            }
            if entity == "account" {
                if self.mapping.get(id).is_some_and(|value| value != &target) {
                    return Err("Esta cuenta ya está vinculada a otra cuenta de FinApp.".into());
                }
                self.mapping.insert(id.into(), target.clone());
            }
            if entity == "category"
                && self
                    .input
                    .category_mapping
                    .get(id)
                    .is_some_and(|value| value != &target)
            {
                return Err("Esta categoría ya está vinculada a otra categoría de FinApp.".into());
            }
            self.report.duplicates += 1;
            return Ok(());
        }
        let target = match entity {
            "account" => self.mapping.get(id).cloned().unwrap_or_else(|| id.into()),
            "category" => self.category_target(conn, id, ty)?,
            _ => id.into(),
        };
        let mapped = match entity {
            "account" => self.mapping.contains_key(id),
            "category" => self.input.category_mapping.contains_key(id),
            _ => false,
        };
        let exists: bool = conn
            .query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE user_id=? AND id=?)"),
                params![self.user, target],
                |r| r.get(0),
            )
            .map_err(|e| {
                log_sqlite("source_exists", &self.place(), &e);
                "No se pudo comprobar el ID."
            })?;
        if exists && !mapped {
            return Err(
                "El ID ya existe en FinApp. Si es una cuenta, selecciona su vínculo.".into(),
            );
        }
        if !matches!(
            key(value(row, "moneda")).as_str(),
            "" | "pyg" | "gs" | "gs." | "guaranies"
        ) {
            return Err("Moneda distinta de PYG.".into());
        }
        let mut skipped = false;
        match entity {
            "account" => self.add_account(conn, row, id, &target, ty)?,
            "category" => {
                if !mapped {
                    finance::save_category_in(
                        conn,
                        self.user,
                        &Category {
                            id: target.clone(),
                            name: value(row, "nombre").into(),
                            kind: ty.into(),
                        },
                    )
                    .map_err(finance_error)?;
                }
                if archived(row) {
                    catalog::save(
                        conn,
                        self.user,
                        &catalog::Detail {
                            entity: "category".into(),
                            id: target.clone(),
                            archived: true,
                            ..Default::default()
                        },
                    )
                    .map_err(finance_error)?;
                }
            }
            _ => match key(value(row, "estado")).as_str() {
                "" | "activo" | "activa" | "registrado" | "confirmado" | "aplicado" | "vigente"
                | "ok" => self.add_operation(conn, row, id, ty)?,
                "cancelado" | "anulado" | "eliminado" | "borrado" => {
                    self.report.warnings.push(format!(
                        "Movimiento {id}: omitido por estado {}.",
                        value(row, "estado")
                    ));
                    skipped = true;
                }
                _ => return Err(format!("Revisa el estado «{}».", value(row, "estado"))),
            },
        }
        conn.execute(
            "INSERT INTO import_sources VALUES(?,?,?,?,?)",
            params![self.user, entity, id, target, fingerprint],
        )
        .map_err(|e| {
            log_sqlite("source_save", &self.place(), &e);
            "No se pudo guardar el origen de la fila."
        })?;
        if skipped {
            self.report.skipped += 1;
        } else {
            self.report.added += 1;
        }
        Ok(())
    }
}

fn run(
    conn: &mut rusqlite::Transaction<'_>,
    user: &str,
    sheets: &Sheets,
    input: &Upload,
) -> Result<Preview, rusqlite::Error> {
    let mut import = Import {
        user,
        input,
        mapping: input.mapping.clone(),
        expected: BTreeMap::new(),
        report: Preview::default(),
        sheet: "",
        line: 0,
    };
    let mut seen = BTreeSet::new();
    for (name, rows) in sheets {
        let (entity, id_column) = match *name {
            "cuentas" => ("account", "cuenta_id"),
            "categorias" => ("category", "categoria_id"),
            _ => ("operation", "movimiento_id"),
        };
        let mut ordered: Vec<_> = rows.iter().collect();
        if entity == "operation" {
            ordered.sort_by_key(|(_, r)| kind(&key(value(r, "tipo"))) == "refund");
        }
        for (line, row) in ordered {
            import.sheet = *name;
            import.line = *line;
            let id = value(row, id_column);
            let normalized = key(value(row, "tipo"));
            let ty = kind(&normalized);
            let result = if id.is_empty() || id.len() > 128 || !seen.insert((entity, id)) {
                Err("ID de origen vacío, demasiado largo o repetido.".into())
            } else {
                let save = conn
                    .savepoint()
                    .inspect_err(|e| log_sqlite("savepoint", &import.place(), e))?;
                let result = import.row(&save, entity, row, id, ty);
                if result.is_ok() {
                    save.commit()
                        .inspect_err(|e| log_sqlite("savepoint_commit", &import.place(), e))?;
                }
                result
            };
            if let Err(error) = result {
                import
                    .report
                    .errors
                    .push(format!("{name}, fila {line} ({id}): {error}"));
            }
            if entity == "account" {
                import.report.accounts.push(SourceAccount {
                    id: id.into(),
                    name: value(row, "nombre").into(),
                    kind: ty.into(),
                    linked: import.mapping.get(id).cloned(),
                });
            }
            if entity == "category" {
                import.report.categories.push(SourceCategory {
                    id: id.into(),
                    name: value(row, "nombre").into(),
                    kind: ty.into(),
                    linked: import.input.category_mapping.get(id).cloned(),
                });
            }
        }
    }
    import.sheet = "balances";
    let balances = finance::balances_in(conn, user, None)
        .inspect_err(|e| log_sqlite("balances", "balance final", e))?;
    for (id, balance) in balances {
        if balance.abs() > i128::from(finance::SAFE) {
            import.report.errors.push(format!(
                "El saldo calculado de {id} excede el rango admitido."
            ));
        }
        if let Some(expected) = import
            .expected
            .get(&id)
            .filter(|n| i128::from(**n) != balance)
        {
            import.report.warnings.push(format!(
                "Cuenta {id}: saldo esperado {expected}, saldo calculado {balance}."
            ));
            if balance.abs() <= i128::from(finance::SAFE) {
                let difference = i128::from(*expected) - balance;
                if let (Ok(calculated), Ok(difference)) =
                    (i64::try_from(balance), i64::try_from(difference))
                {
                    import.report.differences.push(BalanceDiscrepancy {
                        account_id: id.clone(),
                        declared_balance: *expected,
                        calculated_balance: calculated,
                        difference,
                    });
                }
            }
        }
        import.report.balances.insert(id, balance.to_string());
    }
    Ok(import.report)
}

fn db_error(error: rusqlite::Error) -> Problem {
    db_error_at("db", "import/export", error)
}

fn db_error_at(stage: &str, place: &str, error: rusqlite::Error) -> Problem {
    log_sqlite(stage, place, &error);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "No se pudo consultar la base de datos.".into(),
    )
}

async fn import(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Upload>,
) -> Result<Json<Preview>, Problem> {
    let user = finance::bound_user(&state.db, &headers).map_err(|e| (e, String::new()))?;
    let invalid = |message| (StatusCode::BAD_REQUEST, message);
    let file = STANDARD
        .decode(&input.file)
        .map_err(|_| invalid("Archivo inválido.".into()))?;
    tokio::task::spawn_blocking(move || {
        let started = std::time::Instant::now();
        let sheets = book(&file).map_err(invalid)?;
        let mut conn = state.db.0.lock().map_err(|_| {
            eprintln!("finapp mutex stage=import_lock place=importación error=poisoned");
            (StatusCode::INTERNAL_SERVER_ERROR, String::new())
        })?;
        let mut tx = conn
            .transaction()
            .map_err(|e| db_error_at("import_begin", "importación", e))?;
        let mut report = run(&mut tx, &user.id, &sheets, &input)
            .map_err(|e| db_error_at("import_run", "importación", e))?;
        let mut hash = Sha256::new();
        hash.update(Sha256::digest(&file));
        hash.update(
            serde_json::to_vec(&(
                &user.id,
                &input.mapping,
                &input.category_mapping,
                &input.operation_mapping,
                input.include_before_cutoff,
                &report,
            ))
            .map_err(|_| {
                eprintln!("finapp stage=hash place=importación error=serialization");
                (StatusCode::INTERNAL_SERVER_ERROR, String::new())
            })?,
        );
        let token = hex::encode(hash.finalize());
        if let Some(reviewed) = &input.preview_token {
            if reviewed != &token || !report.errors.is_empty() {
                return Err((
                    StatusCode::CONFLICT,
                    "La vista previa cambió. Vuelve a revisarla antes de importar.".into(),
                ));
            }
            if !report.differences.is_empty() && input.ack_saldo.as_deref() != Some(token.as_str())
            {
                return Err((
                    StatusCode::CONFLICT,
                    "Hay diferencias de saldo. Marca la aceptación de la vista previa antes de importar.".into(),
                ));
            }
            tx.commit()
                .map_err(|e| db_error_at("import_commit", "confirmación", e))?;
            report.committed = true;
        }
        eprintln!(
            "finapp import stage=summary added={} duplicates={} skipped={} errors={} elapsed_ms={}",
            report.added,
            report.duplicates,
            report.skipped,
            report.errors.len(),
            started.elapsed().as_millis()
        );
        report.token = token;
        Ok(Json(report))
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, String::new()))?
}

#[derive(Deserialize)]
struct ExportQuery {
    month: Option<String>,
}

fn csv_text(text: &str) -> String {
    if text.trim_start().starts_with(['=', '+', '-', '@']) || text.starts_with(['\t', '\r', '\n']) {
        format!("'{text}")
    } else {
        text.into()
    }
}

async fn export(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(kind): Path<String>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, Problem> {
    let user = finance::bound_user(&state.db, &headers).map_err(|e| (e, String::new()))?;
    let conn = state
        .db
        .0
        .lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, String::new()))?;
    let mut csv = csv::Writer::from_writer(vec![0xef, 0xbb, 0xbf]);
    let internal = |_: csv::Error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "No se pudo generar el CSV.".into(),
        )
    };
    match kind.as_str() {
        "cuentas" => {
            csv.write_record([
                "ID",
                "Nombre",
                "Tipo",
                "Entidad",
                "Estado",
                "Saldo inicial (PYG)",
                "Fecha de corte",
                "Saldo (PYG)",
            ])
            .map_err(internal)?;
            let balances = finance::balances_in(&conn, &user.id, None).map_err(db_error)?;
            let mut stmt=conn.prepare("SELECT a.id,a.name,a.kind,COALESCE(d.institution,''),COALESCE(d.archived,0),
                a.initial_balance,COALESCE(d.cutoff_date,'') FROM finance_accounts a LEFT JOIN catalog_details d
                ON a.user_id=d.user_id AND d.entity='account' AND a.id=d.id WHERE a.user_id=? ORDER BY a.name,a.id").map_err(db_error)?;
            let mut rows = stmt.query([&user.id]).map_err(db_error)?;
            while let Some(row) = rows.next().map_err(db_error)? {
                let id: String = row.get(0).map_err(db_error)?;
                let text = |i| {
                    row.get::<_, String>(i)
                        .map(|s| csv_text(&s))
                        .map_err(db_error)
                };
                csv.write_record([
                    csv_text(&id),
                    text(1)?,
                    text(2)?,
                    text(3)?,
                    if row.get::<_, bool>(4).map_err(db_error)? {
                        "Archivada".into()
                    } else {
                        "Activa".into()
                    },
                    row.get::<_, i64>(5).map_err(db_error)?.to_string(),
                    text(6)?,
                    balances[&id].to_string(),
                ])
                .map_err(internal)?;
            }
        }
        "movimientos" => {
            csv.write_record([
                "ID",
                "Fecha",
                "Tipo",
                "Cuenta",
                "Categoría",
                "Tarjeta",
                "Destino",
                "Movimiento original",
                "Importe (PYG)",
                "Descripción",
                "Notas",
            ])
            .map_err(internal)?;
            let mut stmt=conn.prepare("SELECT id,date,kind,account_id,category_id,card_account_id,destination_account_id,
                original_operation_id,amount,description,notes FROM finance_operations WHERE user_id=? AND deleted=0 ORDER BY date,id").map_err(db_error)?;
            let mut rows = stmt.query([&user.id]).map_err(db_error)?;
            while let Some(row) = rows.next().map_err(db_error)? {
                let mut record = Vec::new();
                for column in 0..11 {
                    if column == 8 {
                        record.push(row.get::<_, i64>(column).map_err(db_error)?.to_string());
                    } else {
                        record.push(csv_text(
                            &row.get::<_, Option<String>>(column)
                                .map_err(db_error)?
                                .unwrap_or_default(),
                        ));
                    }
                }
                csv.write_record(record).map_err(internal)?;
            }
        }
        "informe" => {
            let month = query
                .month
                .as_deref()
                .filter(|s| s.len() == 7 && finance::valid_date(&format!("{s}-01")))
                .ok_or_else(|| (StatusCode::BAD_REQUEST, "Selecciona un mes.".into()))?;
            let report = planning::report(&conn, &user.id, month)
                .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, String::new()))?;
            csv.write_record(["Mes", "Concepto", "Detalle", "Importe (PYG)"])
                .map_err(internal)?;
            for (key, label) in [
                ("income", "Ingresos"),
                ("expense", "Gastos netos"),
                ("net", "Resultado"),
                ("cash_flow", "Flujo de efectivo"),
                ("net_worth", "Patrimonio"),
                ("loan_principal", "Capital de préstamos pagado"),
                ("loan_interest", "Interés de préstamos"),
                ("loan_fees", "Comisiones de préstamos"),
            ] {
                if let Some(amount) = report[key].as_str() {
                    csv.write_record([month, label, "", amount])
                        .map_err(internal)?;
                }
            }
            for (field, label) in [
                ("categories", "Gastos por categoría"),
                ("accounts", "Gastos por cuenta"),
            ] {
                if let Some(items) = report[field].as_object() {
                    for (id, amount) in items {
                        csv.write_record([
                            month,
                            label,
                            &csv_text(id),
                            amount.as_str().unwrap_or_default(),
                        ])
                        .map_err(internal)?;
                    }
                }
            }
        }
        _ => return Err((StatusCode::NOT_FOUND, String::new())),
    }
    let body = csv
        .into_inner()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, String::new()))?;
    let disposition = format!("attachment; filename=finapp-{kind}.csv");
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (header::CONTENT_DISPOSITION, disposition.as_str()),
        ],
        body,
    )
        .into_response())
}

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/import/xlsx",
            post(import).layer(DefaultBodyLimit::max(
                (MAX_COMPRESSED * 4).div_ceil(3) + 256 * 1024,
            )),
        )
        .route("/api/export/{kind}", get(export))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_dates_require_a_four_digit_year() {
        assert_eq!(parse_date("26/1/2026").unwrap(), "2026-01-26");
        assert_eq!(parse_date("2026-01-26").unwrap(), "2026-01-26");
        assert_eq!(parse_date("2024-02-29T10:30").unwrap(), "2024-02-29");
        assert!(
            parse_date("26/1/26")
                .unwrap_err()
                .contains("cuatro dígitos")
        );
        assert!(
            parse_date("2026/1/26")
                .unwrap_err()
                .contains("cuatro dígitos")
        );
    }

    #[test]
    fn invalid_text_dates_are_rejected_with_calendar_limits() {
        assert!(parse_date("31/2/2026").is_err());
        assert!(parse_date("29/2/2023").is_err());
        assert!(parse_date("2023-02-29").is_err());
        assert!(parse_date("2026-13-01").is_err());
        assert_eq!(parse_date("1/1/2026").unwrap(), "2026-01-01");
    }
}
