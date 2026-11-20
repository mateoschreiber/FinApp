use finapp::{app, db::Database, valid_origin};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::var("FINAPP_DB_PATH").unwrap_or_else(|_| "finapp.sqlite".into());
    let db = Database::open(path)?;
    if !db.is_initialized()? {
        let username = std::env::var("FINAPP_ADMIN_USERNAME")
            .map_err(|_| "FINAPP_ADMIN_USERNAME is required on first startup")?;
        let password = std::env::var("FINAPP_ADMIN_PASSWORD")
            .map_err(|_| "FINAPP_ADMIN_PASSWORD is required on first startup")?;
        db.bootstrap_admin(&username, &password)?;
    }
    let bind = std::env::var("FINAPP_BIND").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let origin = std::env::var("FINAPP_ORIGIN").map_err(|_| "FINAPP_ORIGIN is required")?;
    if !valid_origin(&origin) {
        return Err("FINAPP_ORIGIN must be https://host, or http://localhost, http://127.0.0.1 or http://[::1] (lowercase, no path, query, credentials or default port)".into());
    }
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app(db, origin)).await?;
    Ok(())
}
