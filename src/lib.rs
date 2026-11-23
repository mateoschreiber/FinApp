pub mod auth;
pub mod catalog;
pub mod credit;
pub mod db;
pub mod exchange;
pub mod finance;
pub mod planning;
pub mod reconciliation;

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Method, Request, StatusCode, Uri, header},
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
        let authority = request.uri().authority().map(|a| a.as_str()).or_else(|| {
            request
                .headers()
                .get(header::HOST)
                .and_then(|h| h.to_str().ok())
        });
        let origin_matches = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|origin| origin_matches(origin, &state.origin, authority));
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

fn origin_matches(origin: &str, configured: &str, authority: Option<&str>) -> bool {
    if origin == configured {
        return true;
    }
    // A local browser can address the same HTTP listener by any loopback alias.
    // Require the actual request authority to match the browser's Origin.
    fn loopback_http(origin: &str) -> Option<Uri> {
        if !origin.starts_with("http://") || !valid_origin(origin) {
            return None;
        }
        origin.parse().ok()
    }
    let (Some(origin), Some(configured)) = (loopback_http(origin), loopback_http(configured))
    else {
        return false;
    };
    origin.port_u16() == configured.port_u16()
        && origin.authority().map(|a| a.as_str()) == authority
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

pub fn valid_origin(origin: &str) -> bool {
    let Ok(uri) = origin.parse::<axum::http::Uri>() else {
        return false;
    };
    let authority = uri.authority().map_or("", |a| a.as_str());
    let has_port = authority.contains(':') && !authority.ends_with(']');
    let port_valid = !has_port || uri.port_u16().is_some();
    let plain = origin == origin.to_ascii_lowercase()
        && port_valid
        && uri.authority().is_some()
        && uri.path() == "/"
        && uri.query().is_none()
        && !origin.ends_with('/')
        && !origin.contains('@')
        && !origin.contains('#')
        && !origin.ends_with(':');
    plain
        && match uri.scheme_str() {
            Some("https") => uri.port_u16() != Some(443),
            Some("http") => {
                uri.port_u16() != Some(80)
                    && matches!(uri.host(), Some("localhost" | "127.0.0.1" | "[::1]"))
            }
            _ => false,
        }
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
        .route("/api/me", get(auth::me).patch(auth::update_me))
        .route("/api/users", get(auth::list_users).post(auth::create_user))
        .route(
            "/api/users/{id}",
            axum::routing::patch(auth::update_user).delete(auth::delete_user),
        )
        .merge(finance::routes())
        .merge(catalog::routes())
        .merge(credit::routes())
        .merge(exchange::routes())
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
