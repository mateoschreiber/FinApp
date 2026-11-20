//! Import previews and confirmations validate the same rows and run against a transaction.
use crate::{
    AppState, catalog,
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
use std::{collections::{BTreeMap, BTreeSet}, io::{Cursor, Read}};

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
    #[serde(default)]
    include_before_cutoff: bool,
    preview_token: Option<String>,
}

#[derive(Default, Serialize)]
struct Preview {
    accounts: Vec<SourceAccount>,
    added: usize,
    duplicates: usize,
    skipped: usize,
    errors: Vec<String>,
    warnings: Vec<String>,
    balances: BTreeMap<String, String>,
    token: String,
    committed: bool,
}
#[derive(Serialize)]
struct SourceAccount { id: String, name: String, kind: String, linked: Option<String> }

fn key(s: &str) -> String {
    s.trim().to_lowercase().replace('á',"a").replace('é',"e").replace('í',"i")
        .replace('ó',"o").replace('ú',"u").replace([' ','-'],"_")
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
    hex::encode(Sha256::digest(serde_json::to_vec(row).expect("text-only row")))
}

fn book(bytes: &[u8]) -> Result<Sheets, String> {
    let invalid = || "El archivo debe ser un libro XLSX válido.".to_string();
    if bytes.len() > MAX_COMPRESSED {
        return Err("El XLSX excede 5 MiB.".into());
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| invalid())?;
    if zip.len() > 512 || zip.decompressed_size().is_none_or(|size| size > MAX_EXPANDED as u128) {
        return Err("El libro excede el tamaño admitido.".into());
    }
    let mut expanded = 0;
    for index in 0..zip.len() {
        let mut file = zip.by_index(index).map_err(|_| invalid())?;
        let limit = MAX_EXPANDED - expanded + 1;
        expanded += std::io::copy(&mut file.by_ref().take(limit as u64), &mut std::io::sink())
            .map_err(|_| invalid())? as usize;
        if expanded > MAX_EXPANDED { return Err("El libro excede el tamaño admitido.".into()); }
    }
    let mut workbook: Xlsx<_> = Xlsx::new(Cursor::new(bytes)).map_err(|_| invalid())?;
    let mut result = Vec::new();
    for (sheet, required, max_rows) in [
        ("cuentas", &["cuenta_id","tipo","nombre"][..], MAX_CATALOG_ROWS),
        ("categorias", &["categoria_id","tipo","nombre"][..], MAX_CATALOG_ROWS),
        ("movimientos", &["movimiento_id","tipo","fecha","monto"][..], MAX_MOVEMENTS),
    ] {
        let name = workbook.sheet_names().iter().find(|n| key(n)==sheet)
            .cloned().ok_or_else(|| format!("Falta la hoja {sheet}."))?;
        let mut reader = workbook.worksheet_cells_reader(&name).map_err(|_| format!("No se puede leer {name}."))?;
        let mut header_row = None;
        let mut headers = BTreeMap::new();
        let mut rows: BTreeMap<u32, Row> = BTreeMap::new();
        while let Some(cell) = reader.next_cell().map_err(|_| format!("Celda inválida en {name}."))? {
            let (row, column) = cell.get_position();
            let value = cell.get_value();
            if matches!(value, DataRef::Empty) { continue; }
            let first = *header_row.get_or_insert(row);
            if row>first+max_rows || column>=MAX_COLUMNS {
                return Err(format!("La hoja {name} excede el límite de filas o columnas."));
            }
            if row==first {
                let label=key(&Data::from(value.clone()).to_string());
                if !label.is_empty() && headers.values().any(|v| v==&label) {
                    return Err(format!("Encabezado repetido en {name}: {label}."));
                }
                headers.insert(column,label);
            } else if let Some(label)=headers.get(&column).filter(|h| !h.is_empty()) {
                let text = match value {
                    DataRef::DateTime(_) => value.as_date().map(|date| date.to_string()).ok_or_else(|| format!("Fecha inválida en {name}, fila {}.",row+1))?,
                    _ => Data::from(value.clone()).to_string().trim().to_owned(),
                };
                rows.entry(row+1).or_default().insert(label.clone(),text);
            }
        }
        for column in required {
            if !headers.values().any(|value| value==column) {
                return Err(format!("Falta la columna {column} en {name}."));
            }
        }
        result.push((sheet,rows.into_iter().collect()));
    }
    Ok(result)
}

fn number(s: &str) -> Result<i64, String> {
    s.parse::<i64>().ok().filter(|value| value.unsigned_abs() <= finance::SAFE as u64)
        .ok_or_else(|| format!("Importe o saldo inválido «{s}». Usa guaraníes enteros."))
}
fn opening_amount(s: &str) -> Result<i64, String> { if s.is_empty() { Ok(0) } else { number(s) } }

fn parse_date(s: &str) -> Result<String, String> {
    let date = if let Some((day, rest)) = s.split_once('/') {
        let (month, year)=rest.split_once('/').ok_or_else(||format!("Fecha inválida «{s}»."))?;
        format!("{year:0>4}-{month:0>2}-{day:0>2}")
    } else {
        s.split('T').next().unwrap_or(s).to_owned()
    };
    if finance::valid_date(&date) { Ok(date) } else { Err(format!("Fecha inválida «{s}». Usa aaaa-mm-dd.")) }
}
fn archived(row: &Row) -> bool {
    matches!(key(value(row,"activa")).as_str(),"false"|"no"|"0"|"inactiva")
}
fn finance_error(error: finance::Error) -> String {
    match error {
        finance::Error::Invalid => "Datos inválidos o referencias incompatibles. Revisa la cuenta, categoría y tipo.",
        finance::Error::DuplicateRequest => "Esta solicitud ya existe con otros datos.",
        finance::Error::Database => "No se pudo guardar esta fila.",
    }.into()
}

struct Import<'a> {
    user: &'a str,
    input: &'a Upload,
    mapping: BTreeMap<String, String>,
    expected: BTreeMap<String, i64>,
    report: Preview,
}

impl Import<'_> {
    fn add_account(&mut self, conn:&Connection, row:&Row, id:&str, target:&str, ty:&str) -> Result<(),String> {
        let balance=opening_amount(value(row,"saldo_inicial"))?;
        if self.mapping.contains_key(id) {
            let (actual, initial):(String,i64)=conn.query_row(
                "SELECT kind,initial_balance FROM finance_accounts WHERE user_id=? AND id=?",
                params![self.user,target], |r| Ok((r.get(0)?,r.get(1)?))
            ).map_err(|_| "La cuenta vinculada no existe.")?;
            if actual!=ty { return Err("La cuenta vinculada tiene un tipo diferente.".into()); }
            if initial!=balance {
                self.report.warnings.push(format!("Cuenta {id}: saldo inicial en FinApp {initial}, saldo inicial del XLSX {balance}."));
            }
        } else {
            finance::save_account_in(conn,self.user,&Account {
                id:target.into(), name:value(row,"nombre").into(), kind:ty.into(), initial_balance:balance,
            }).map_err(finance_error)?;
            let day=|field| optional(value(row,field)).map(|s| s.parse::<u8>().map_err(|_|"Día de tarjeta inválido.")).transpose();
            catalog::save(conn,self.user,&catalog::Detail {
                entity:"account".into(), id:target.into(), archived:archived(row),
                institution:value(row,"entidad").into(), notes:value(row,"notas").into(),
                cutoff_date:optional(value(row,"fecha_corte")).as_deref().map(parse_date).transpose()?,
                closing_day:day("dia_cierre")?, due_day:day("dia_vencimiento")?,
            }).map_err(finance_error)?;
        }
        if let Some(saldo)=optional(value(row,"saldo_actual")) {
            self.expected.insert(target.into(),number(&saldo)?);
        }
        Ok(())
    }

    fn add_operation(&mut self, conn:&Connection, row:&Row, id:&str, ty:&str) -> Result<(),String> {
        let date=parse_date(value(row,"fecha"))?;
        let amount=number(value(row,"monto"))?;
        let mapped=|field| {
            let id=value(row,field);
            self.mapping.get(id).cloned().or_else(||optional(id))
        };
        let dest=mapped("cuenta_destino_id");
        let origin=mapped("cuenta_origen_id");
        let account=if matches!(ty,"income"|"loan_draw"|"refund") {dest.clone().or(origin)} else {origin};
        let mut category=optional(value(row,"categoria_id"));
        let original=optional(value(row,"movimiento_relacionado_id"));
        if ty=="refund" && category.is_none() {
            category=conn.query_row("SELECT category_id FROM finance_operations WHERE user_id=? AND id=?",
                params![self.user,original], |r|r.get(0)).optional().map_err(|_|"No se pudo leer el gasto original.")?.flatten();
        }
        for acct in BTreeSet::from_iter([account.as_deref(),dest.as_deref()].into_iter().flatten()) {
            let cutoff=catalog::cutoff(conn,self.user,acct).map_err(|_|"No se pudo consultar la fecha de corte.")?;
            if cutoff.is_some_and(|cutoff| date<cutoff) {
                self.report.warnings.push(format!("Movimiento {id}: anterior a la fecha de corte de {acct}."));
                if !self.input.include_before_cutoff {
                    return Err("Confirma la importación del histórico anterior al corte.".into());
                }
            }
        }
        let mut change=Change {
            request_id:format!("xlsx-{}",fingerprint(row)),
            id:id.into(), expected_version:0, deleted:false, kind:Some(ty.into()), account_id:account,
            category_id:if matches!(ty,"income"|"expense"|"refund"){category.clone()}else{None},
            card_account_id:if ty=="payment"{dest.clone()}else{None},
            destination_account_id:if ty=="transfer"{dest}else{None},
            original_operation_id:if ty=="refund"{original}else{None},
            amount:Some(amount), date:Some(date), description:optional(value(row,"descripcion")), notes:optional(value(row,"notas")),
        };
        let apply=|change:&Change| -> Result<(),String> { match finance::apply_import_in(conn,self.user,change).map_err(finance_error)? {
            Outcome::Applied(_) => Ok(()),
            Outcome::Conflict(_) => Err("El ID del movimiento ya existe.".into()),
        }};
        if ty=="loan_repayment" {
            let interest=opening_amount(value(row,"interes"))?;
            let fees=opening_amount(value(row,"comisiones"))?;
            let principal=if value(row,"capital").is_empty(){amount-interest-fees}else{number(value(row,"capital"))?};
            if [principal,interest,fees].iter().any(|n| *n<0) || principal+interest+fees!=amount || amount<=0 {
                return Err("El capital, los intereses y las comisiones no suman el importe del pago.".into());
            }
            let first=if principal>0 { "capital" } else if interest>0 { "interes" } else { "comision" };
            for (part, amount) in [("capital",principal),("interes",interest),("comision",fees)] {
                if amount==0 { continue; }
                change.id=if part==first {id.into()} else {format!("xlsx-{part}-{}",fingerprint(row))};
                change.request_id=format!("xlsx-{part}-{}",fingerprint(row));
                change.amount=Some(amount);
                change.kind=Some(if part=="capital" { "loan_repayment" } else { "expense" }.into());
                change.category_id=if part=="capital" {None} else {category.clone()};
                if part!="capital" { change.description=Some(format!("{part} · {}",value(row,"descripcion"))); }
                apply(&change)?;
            }
        } else {
            apply(&change)?;
        }
        Ok(())
    }

    fn row(&mut self, conn:&Connection, entity:&str, row:&Row, id:&str, ty:&str) -> Result<(),String> {
        let table=match entity {"account"=>"finance_accounts","category"=>"finance_categories",_=>"finance_operations"};
        let fingerprint=fingerprint(row);
        let prior=conn.query_row("SELECT target_id,fingerprint FROM import_sources WHERE user_id=? AND entity=? AND source_id=?",
            params![self.user,entity,id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))
            .optional().map_err(|_|"No se pudo consultar el historial de importación.")?;
        if let Some((target, hash))=prior {
            if fingerprint!=hash {return Err("El ID importado previamente tiene datos diferentes.".into());}
            if entity=="account" {
                if self.mapping.get(id).is_some_and(|value| value!=&target) {
                    return Err("Esta cuenta ya está vinculada a otra cuenta de FinApp.".into());
                }
                self.mapping.insert(id.into(),target);
            }
            self.report.duplicates+=1;
            return Ok(());
        }
        let target=if entity=="account" { self.mapping.get(id).cloned().unwrap_or_else(||id.into()) } else {id.into()};
        let mapped=entity=="account" && self.mapping.contains_key(id);
        let exists:bool=conn.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE user_id=? AND id=?)"),
            params![self.user,target], |r|r.get(0)).map_err(|_|"No se pudo comprobar el ID.")?;
        if exists && !mapped { return Err("El ID ya existe en FinApp. Si es una cuenta, selecciona su vínculo.".into()); }
        if !matches!(key(value(row,"moneda")).as_str(),""|"pyg"|"gs"|"gs."|"guaranies") {
            return Err("Moneda distinta de PYG.".into());
        }
        let mut skipped=false;
        match entity {
            "account" => self.add_account(conn,row,id,&target,ty)?,
            "category" => {
                finance::save_category_in(conn,self.user,&Category {id:id.into(),name:value(row,"nombre").into(),kind:ty.into()})
                    .map_err(finance_error)?;
                if archived(row) {
                    catalog::save(conn,self.user,&catalog::Detail {entity:"category".into(),id:id.into(),archived:true,..Default::default()})
                        .map_err(finance_error)?;
                }
            },
            _ => match key(value(row,"estado")).as_str() {
                ""|"activo"|"activa"|"registrado"|"confirmado"|"aplicado"|"vigente"|"ok" => self.add_operation(conn,row,id,ty)?,
                "cancelado"|"anulado"|"eliminado"|"borrado" => {
                    self.report.warnings.push(format!("Movimiento {id}: omitido por estado {}.",value(row,"estado")));
                    skipped=true;
                }
                _ => return Err(format!("Revisa el estado «{}».",value(row,"estado"))),
            },
        }
        conn.execute("INSERT INTO import_sources VALUES(?,?,?,?,?)",params![self.user,entity,id,target,fingerprint])
            .map_err(|_|"No se pudo guardar el origen de la fila.")?;
        if skipped {self.report.skipped+=1;} else {self.report.added+=1;}
        Ok(())
    }
}

fn run(conn:&mut rusqlite::Transaction<'_>,user:&str,sheets:&Sheets,input:&Upload) -> Result<Preview,rusqlite::Error> {
    let mut import=Import {user,input,mapping:input.mapping.clone(),expected:BTreeMap::new(),report:Preview::default()};
    let mut seen=BTreeSet::new();
    for (name,rows) in sheets {
        let (entity,id_column)=match *name {
            "cuentas"=>("account","cuenta_id"), "categorias"=>("category","categoria_id"), _=>("operation","movimiento_id"),
        };
        let mut ordered:Vec<_>=rows.iter().collect();
        if entity=="operation" { ordered.sort_by_key(|(_,r)| kind(&key(value(r,"tipo")))=="refund"); }
        for (line,row) in ordered {
            let id=value(row,id_column);
            let normalized=key(value(row,"tipo"));
            let ty=kind(&normalized);
            let result=if id.is_empty() || id.len()>128 || !seen.insert((entity,id)) {
                Err("ID de origen vacío, demasiado largo o repetido.".into())
            } else {
                let save=conn.savepoint()?;
                let result=import.row(&save,entity,row,id,ty);
                if result.is_ok() {save.commit()?;}
                result
            };
            if let Err(error)=result {import.report.errors.push(format!("{name}, fila {line} ({id}): {error}"));}
            if entity=="account" {
                import.report.accounts.push(SourceAccount {
                    id:id.into(),name:value(row,"nombre").into(),kind:ty.into(),linked:import.mapping.get(id).cloned(),
                });
            }
        }
    }
    for (id,balance) in finance::balances_in(conn,user,None)? {
        if balance.abs()>i128::from(finance::SAFE) {
            import.report.errors.push(format!("El saldo calculado de {id} excede el rango admitido."));
        }
        if let Some(expected)=import.expected.get(&id).filter(|n| i128::from(**n)!=balance) {
            import.report.warnings.push(format!("Cuenta {id}: saldo esperado {expected}, saldo calculado {balance}."));
        }
        import.report.balances.insert(id,balance.to_string());
    }
    Ok(import.report)
}

fn db_error(_: rusqlite::Error) -> Problem {
    (StatusCode::INTERNAL_SERVER_ERROR,"No se pudo consultar la base de datos.".into())
}

async fn import(State(state):State<AppState>,headers:HeaderMap,Json(input):Json<Upload>) -> Result<Json<Preview>,Problem> {
    let user=finance::bound_user(&state.db,&headers).map_err(|e|(e,String::new()))?;
    let invalid=|message|(StatusCode::BAD_REQUEST,message);
    let file=STANDARD.decode(&input.file).map_err(|_|invalid("Archivo inválido.".into()))?;
    tokio::task::spawn_blocking(move || {
        let sheets=book(&file).map_err(invalid)?;
        let mut conn=state.db.0.lock().map_err(|_|(StatusCode::INTERNAL_SERVER_ERROR,String::new()))?;
        let mut tx=conn.transaction().map_err(db_error)?;
        let mut report=run(&mut tx,&user.id,&sheets,&input).map_err(db_error)?;
        let mut hash=Sha256::new();
        hash.update(Sha256::digest(&file));
        hash.update(serde_json::to_vec(&(&user.id,&input.mapping,input.include_before_cutoff,&report)).map_err(|_|(StatusCode::INTERNAL_SERVER_ERROR,String::new()))?);
        let token=hex::encode(hash.finalize());
        if let Some(reviewed)=&input.preview_token {
            if reviewed!=&token || !report.errors.is_empty() {
                return Err((StatusCode::CONFLICT,"La vista previa cambió. Vuelve a revisarla antes de importar.".into()));
            }
            tx.commit().map_err(db_error)?;
            report.committed=true;
        }
        report.token=token;
        Ok(Json(report))
    }).await.map_err(|_|(StatusCode::INTERNAL_SERVER_ERROR,String::new()))?
}

#[derive(Deserialize)]
struct ExportQuery { month: Option<String> }

fn csv_text(text:&str) -> String {
    if text.trim_start().starts_with(['=','+','-','@']) || text.starts_with(['\t','\r','\n']) {
        format!("'{text}")
    } else {text.into()}
}

async fn export(State(state):State<AppState>,headers:HeaderMap,Path(kind):Path<String>,Query(query):Query<ExportQuery>) -> Result<Response,Problem> {
    let user=finance::bound_user(&state.db,&headers).map_err(|e|(e,String::new()))?;
    let conn=state.db.0.lock().map_err(|_|(StatusCode::INTERNAL_SERVER_ERROR,String::new()))?;
    let mut csv=csv::Writer::from_writer(vec![0xef,0xbb,0xbf]);
    let internal=|_:csv::Error|(StatusCode::INTERNAL_SERVER_ERROR,"No se pudo generar el CSV.".into());
    match kind.as_str() {
        "cuentas" => {
            csv.write_record(["ID","Nombre","Tipo","Entidad","Estado","Saldo inicial (PYG)","Fecha de corte","Saldo (PYG)"]).map_err(internal)?;
            let balances=finance::balances_in(&conn,&user.id,None).map_err(db_error)?;
            let mut stmt=conn.prepare("SELECT a.id,a.name,a.kind,COALESCE(d.institution,''),COALESCE(d.archived,0),
                a.initial_balance,COALESCE(d.cutoff_date,'') FROM finance_accounts a LEFT JOIN catalog_details d
                ON a.user_id=d.user_id AND d.entity='account' AND a.id=d.id WHERE a.user_id=? ORDER BY a.name,a.id").map_err(db_error)?;
            let mut rows=stmt.query([&user.id]).map_err(db_error)?;
            while let Some(row)=rows.next().map_err(db_error)? {
                let id:String=row.get(0).map_err(db_error)?;
                let text=|i|row.get::<_,String>(i).map(|s|csv_text(&s)).map_err(db_error);
                csv.write_record([csv_text(&id),text(1)?,text(2)?,text(3)?,if row.get::<_,bool>(4).map_err(db_error)?{"Archivada".into()}else{"Activa".into()},
                    row.get::<_,i64>(5).map_err(db_error)?.to_string(),text(6)?,balances[&id].to_string()]).map_err(internal)?;
            }
        }
        "movimientos" => {
            csv.write_record(["ID","Fecha","Tipo","Cuenta","Categoría","Tarjeta","Destino","Movimiento original","Importe (PYG)","Descripción","Notas"]).map_err(internal)?;
            let mut stmt=conn.prepare("SELECT id,date,kind,account_id,category_id,card_account_id,destination_account_id,
                original_operation_id,amount,description,notes FROM finance_operations WHERE user_id=? AND deleted=0 ORDER BY date,id").map_err(db_error)?;
            let mut rows=stmt.query([&user.id]).map_err(db_error)?;
            while let Some(row)=rows.next().map_err(db_error)? {
                let mut record=Vec::new();
                for column in 0..11 {
                    if column==8 {record.push(row.get::<_,i64>(column).map_err(db_error)?.to_string());}
                    else {record.push(csv_text(&row.get::<_,Option<String>>(column).map_err(db_error)?.unwrap_or_default()));}
                }
                csv.write_record(record).map_err(internal)?;
            }
        }
        "informe" => {
            let month=query.month.as_deref().filter(|s| s.len()==7 && finance::valid_date(&format!("{s}-01")))
                .ok_or_else(||(StatusCode::BAD_REQUEST,"Selecciona un mes.".into()))?;
            let report=planning::report(&conn,&user.id,month).map_err(|_|(StatusCode::INTERNAL_SERVER_ERROR,String::new()))?;
            csv.write_record(["Mes","Concepto","Detalle","Importe (PYG)"]).map_err(internal)?;
            for (key,label) in [("income","Ingresos"),("expense","Gastos netos"),("net","Resultado"),("cash_flow","Flujo de efectivo"),
                ("net_worth","Patrimonio"),("loan_principal","Capital de préstamos pagado"),("loan_interest","Interés de préstamos"),("loan_fees","Comisiones de préstamos")] {
                if let Some(amount)=report[key].as_str() { csv.write_record([month,label,"",amount]).map_err(internal)?; }
            }
            for (field,label) in [("categories","Gastos por categoría"),("accounts","Gastos por cuenta")] {
                if let Some(items)=report[field].as_object() {
                    for (id, amount) in items {
                        csv.write_record([month,label,&csv_text(id),amount.as_str().unwrap_or_default()]).map_err(internal)?;
                    }
                }
            }
        }
        _ => return Err((StatusCode::NOT_FOUND,String::new())),
    }
    let body=csv.into_inner().map_err(|_|(StatusCode::INTERNAL_SERVER_ERROR,String::new()))?;
    let disposition=format!("attachment; filename=finapp-{kind}.csv");
    Ok(([(header::CONTENT_TYPE,"text/csv; charset=utf-8"),(header::CONTENT_DISPOSITION,disposition.as_str())],body).into_response())
}

pub(crate) fn routes()->Router<AppState> {
    Router::new()
        .route("/api/import/xlsx",post(import).layer(DefaultBodyLimit::max((MAX_COMPRESSED*4).div_ceil(3)+256*1024)))
        .route("/api/export/{kind}",get(export))
}
