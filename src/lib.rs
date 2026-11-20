pub mod auth;
pub mod catalog;
pub mod credit;
pub mod db;
pub mod exchange;
pub mod finance;
pub mod history;
pub mod ledger;
pub mod planning;
pub mod reconciliation;

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Method, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use db::Database;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    attempts: Arc<Mutex<VecDeque<i64>>>,
    origin: Arc<str>,
}

async fn protect_mutations(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        let origin_matches = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|h| h.to_str().ok())
            == Some(state.origin.as_ref());
        let json = request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|s| {
                s.split(';')
                    .next()
                    .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
            });
        if !origin_matches || !json {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    next.run(request).await
}

async fn security_headers(request: Request<Body>, next: Next) -> Response {
    let api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, "default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'; worker-src 'self'".parse().expect("valid CSP"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        "nosniff".parse().expect("valid header"),
    );
    headers.insert(
        header::X_FRAME_OPTIONS,
        "DENY".parse().expect("valid header"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        "no-referrer".parse().expect("valid header"),
    );
    if api {
        headers.insert(
            header::CACHE_CONTROL,
            "no-store".parse().expect("valid header"),
        );
    }
    response
}

pub fn app(db: Database, origin: String) -> Router {
    let state = AppState {
        db,
        attempts: Arc::new(Mutex::new(VecDeque::new())),
        origin: origin.into(),
    };
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/login", post(auth::login))
        .route("/api/logout", post(auth::logout))
        .route("/api/me", get(auth::me))
        .route("/api/users", post(auth::create_user))
        .merge(finance::routes())
        .merge(catalog::routes())
        .merge(credit::routes())
        .merge(exchange::routes())
        .merge(history::routes())
        .merge(planning::routes())
        .merge(reconciliation::routes())
        .fallback_service(tower_http::services::ServeDir::new("static"))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            protect_mutations,
        ))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}
