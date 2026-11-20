use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use finapp::{app, db::Database};
use tower::ServiceExt;

#[tokio::test]
async fn create_user_rejects_missing_or_mismatched_tab_identity() {
    let db = Database::open_memory().unwrap();
    db.bootstrap_admin("admin", "password-1234").unwrap();
    let app = app(db, "https://finanzas.example".to_owned());
    let login = request(
        &app,
        "POST",
        "/api/login",
        r#"{"username":"admin","password":"password-1234"}"#,
        None,
    )
    .await;
    let cookie = login
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    for identity in [None, Some("other-user")] {
        let mut req = Request::builder()
            .method("POST")
            .uri("/api/users")
            .header("content-type", "application/json")
            .header("origin", "https://finanzas.example")
            .header("cookie", &cookie);
        if let Some(identity) = identity {
            req = req.header("x-finapp-user", identity);
        }
        let response = app
            .clone()
            .oneshot(
                req.body(Body::from(
                    r#"{"username":"other","password":"password-1234"}"#,
                ))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    body: &str,
    cookie: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("origin", "https://finanzas.example");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    app.clone()
        .oneshot(builder.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn login_and_admin_user_creation_require_authentication() {
    let db = Database::open_memory().unwrap();
    db.bootstrap_admin("admin", "password-1234").unwrap();
    let user_id: String =
        db.0.lock()
            .unwrap()
            .query_row(
                "SELECT id FROM users WHERE username = ?1",
                ["admin"],
                |row| row.get(0),
            )
            .unwrap();
    let app = app(db, "https://finanzas.example".to_owned());
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/login",
            r#"{"username":"admin","password":"wrong"}"#,
            None
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/users",
            r#"{"username":"other","password":"password-1234"}"#,
            None
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let logged_in = request(
        &app,
        "POST",
        "/api/login",
        r#"{"username":"admin","password":"password-1234"}"#,
        None,
    )
    .await;
    assert_eq!(logged_in.status(), StatusCode::OK);
    let full_cookie = logged_in
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        full_cookie.starts_with("__Host-finapp_session=")
            && full_cookie.contains("Path=/")
            && !full_cookie.contains("Domain=")
            && full_cookie.contains("HttpOnly")
            && full_cookie.contains("Secure")
            && full_cookie.contains("SameSite=Strict")
    );
    let cookie = logged_in
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let legacy_cookie = cookie.replacen("__Host-finapp_session=", "finapp_session=", 1);
    assert_eq!(
        request(&app, "GET", "/api/me", "", Some(&legacy_cookie))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let create_user_request = Request::builder()
        .method("POST")
        .uri("/api/users")
        .header("content-type", "application/json")
        .header("origin", "https://finanzas.example")
        .header("cookie", &cookie)
        .header("x-finapp-user", &user_id)
        .body(Body::from(
            r#"{"username":"other","password":"password-1234"}"#,
        ))
        .unwrap();
    assert_eq!(
        app.clone()
            .oneshot(create_user_request)
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );
    let other = request(
        &app,
        "POST",
        "/api/login",
        r#"{"username":"other","password":"password-1234"}"#,
        None,
    )
    .await;
    let other_cookie = other
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/users",
            r#"{"username":"third","password":"password-1234"}"#,
            Some(&other_cookie)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "GET", "/api/me", "", Some(&cookie))
            .await
            .status(),
        StatusCode::OK
    );
    let logout_request = Request::builder()
        .method("POST")
        .uri("/api/logout")
        .header("content-type", "application/json")
        .header("origin", "https://finanzas.example")
        .header("cookie", &cookie)
        .header("x-finapp-user", user_id)
        .body(Body::from("{}"))
        .unwrap();
    let logout = app.clone().oneshot(logout_request).await.unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    let cleared_cookie = logout
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        cleared_cookie.starts_with("__Host-finapp_session=")
            && cleared_cookie.contains("Path=/")
            && cleared_cookie.contains("Secure")
            && cleared_cookie.contains("Max-Age=0")
    );
    assert_eq!(
        request(&app, "GET", "/api/me", "", Some(&cookie))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn expired_session_is_rejected() {
    let db = Database::open_memory().unwrap();
    db.bootstrap_admin("admin", "password-1234").unwrap();
    let app = app(db.clone(), "https://finanzas.example".to_owned());
    let login = request(
        &app,
        "POST",
        "/api/login",
        r#"{"username":"admin","password":"password-1234"}"#,
        None,
    )
    .await;
    let cookie = login
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    db.0.lock()
        .unwrap()
        .execute("UPDATE sessions SET expires_at=0", [])
        .unwrap();
    assert_eq!(
        request(&app, "GET", "/api/me", "", Some(&cookie))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn mutation_rejects_other_origins() {
    let db = Database::open_memory().unwrap();
    db.bootstrap_admin("admin", "password-1234").unwrap();
    let app = app(db, "https://finanzas.example".to_owned());
    let req = Request::builder()
        .method("POST")
        .uri("/api/login")
        .header("content-type", "application/json")
        .header("origin", "https://evil.example")
        .body(Body::from(
            r#"{"username":"admin","password":"password-1234"}"#,
        ))
        .unwrap();
    assert_eq!(
        app.oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn mutation_rejects_missing_or_null_origin_and_non_json_content() {
    let db = Database::open_memory().unwrap();
    db.bootstrap_admin("admin", "password-1234").unwrap();
    let app = app(db, "https://finanzas.example".to_owned());
    for (origin, content_type) in [
        (None, "application/json"),
        (Some("null"), "application/json"),
        (Some("https://finanzas.example"), "text/plain"),
    ] {
        let mut req = Request::builder()
            .method("POST")
            .uri("/api/login")
            .header("content-type", content_type);
        if let Some(origin) = origin {
            req = req.header("origin", origin);
        }
        let response = app
            .clone()
            .oneshot(
                req.body(Body::from(
                    r#"{"username":"admin","password":"password-1234"}"#,
                ))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn repeated_bootstrap_does_not_change_existing_password() {
    let db = Database::open_memory().unwrap();
    db.bootstrap_admin("admin", "password-1234").unwrap();
    db.bootstrap_admin("admin", "different-pass-1234").unwrap();
    let app = app(db, "https://finanzas.example".to_owned());
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/login",
            r#"{"username":"admin","password":"different-pass-1234"}"#,
            None
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/login",
            r#"{"username":"admin","password":"password-1234"}"#,
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn default_admin_is_created_in_an_empty_database() {
    let db = Database::open_memory().unwrap();
    assert!(!db.is_initialized().unwrap());
    db.bootstrap_admin("admin", "adminhomeadmin").unwrap();
    let app = app(db, "https://finanzas.example".to_owned());
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/login",
            r#"{"username":"admin","password":"adminhomeadmin"}"#,
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn default_admin_does_not_add_a_user_to_an_initialized_database() {
    let db = Database::open_memory().unwrap();
    db.bootstrap_admin("alice", "password-1234").unwrap();
    db.bootstrap_admin("admin", "adminhomeadmin").unwrap();
    let app = app(db, "https://finanzas.example".to_owned());
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/login",
            r#"{"username":"admin","password":"adminhomeadmin"}"#,
            None
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
}

// shortcut: window recovery is tested on admit_login_attempt with fixed instants (src/auth.rs), not over HTTP; inject a clock into AppState if the limiter changes shape.
#[tokio::test]
async fn login_rate_limit_counts_successful_and_failed_attempts() {
    let db = Database::open_memory().unwrap();
    db.bootstrap_admin("admin", "password-1234").unwrap();
    let app = app(db, "https://finanzas.example".to_owned());
    for attempt in 0..20 {
        let (password, expected) = if attempt % 2 == 0 {
            ("password-1234", StatusCode::OK)
        } else {
            ("wrong-password-1234", StatusCode::UNAUTHORIZED)
        };
        let body = format!(r#"{{"username":"admin","password":"{password}"}}"#);
        assert_eq!(
            request(&app, "POST", "/api/login", &body, None)
                .await
                .status(),
            expected,
            "attempt {attempt}"
        );
    }
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/login",
            r#"{"username":"admin","password":"password-1234"}"#,
            None
        )
        .await
        .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn login_rate_limit_counts_unknown_users_and_blocks_every_user() {
    let db = Database::open_memory().unwrap();
    db.bootstrap_admin("admin", "password-1234").unwrap();
    let app = app(db, "https://finanzas.example".to_owned());
    for attempt in 0..20 {
        let body = format!(r#"{{"username":"ghost{attempt}","password":"password-1234"}}"#);
        assert_eq!(
            request(&app, "POST", "/api/login", &body, None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
            "unknown user {attempt}"
        );
    }
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/login",
            r#"{"username":"admin","password":"password-1234"}"#,
            None
        )
        .await
        .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn session_tokens_are_64_hex_characters_and_unique() {
    let db = Database::open_memory().unwrap();
    db.bootstrap_admin("admin", "password-1234").unwrap();
    let app = app(db, "https://finanzas.example".to_owned());
    let mut tokens = Vec::new();
    for _ in 0..2 {
        let login = request(
            &app,
            "POST",
            "/api/login",
            r#"{"username":"admin","password":"password-1234"}"#,
            None,
        )
        .await;
        let cookie = login
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        let token = cookie.strip_prefix("__Host-finapp_session=").unwrap();
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
        tokens.push(token.to_owned());
    }
    assert_ne!(tokens[0], tokens[1]);
}

#[tokio::test]
async fn security_headers_cover_assets_api_and_rejected_requests_without_disabling_static_cache() {
    let app = app(
        Database::open_memory().unwrap(),
        "https://finanzas.example".to_owned(),
    );
    for path in ["/", "/sw.js", "/api/me"] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let headers = response.headers();
        assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
        assert_eq!(headers.get("x-frame-options").unwrap(), "DENY");
        let csp = headers
            .get("content-security-policy")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(csp.contains("default-src 'self'") && csp.contains("frame-ancestors 'none'"));
        if path.starts_with("/api/") {
            assert_eq!(headers.get("cache-control").unwrap(), "no-store");
        } else {
            assert_ne!(
                headers.get("cache-control").and_then(|v| v.to_str().ok()),
                Some("no-store")
            );
        }
    }
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    assert_eq!(
        response.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );
}

#[tokio::test]
async fn logout_requires_user_identity_matching_the_cookie() {
    let db = Database::open_memory().unwrap();
    db.bootstrap_admin("admin", "password-1234").unwrap();
    let user_id: String =
        db.0.lock()
            .unwrap()
            .query_row(
                "SELECT id FROM users WHERE username = ?1",
                ["admin"],
                |row| row.get(0),
            )
            .unwrap();
    let app = app(db, "https://finanzas.example".to_owned());
    let login = request(
        &app,
        "POST",
        "/api/login",
        r#"{"username":"admin","password":"password-1234"}"#,
        None,
    )
    .await;
    let cookie = login
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(
        request(&app, "POST", "/api/logout", "{}", Some(&cookie))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let mismatched = Request::builder()
        .method("POST")
        .uri("/api/logout")
        .header("content-type", "application/json")
        .header("origin", "https://finanzas.example")
        .header("cookie", &cookie)
        .header("x-finapp-user", "different-user")
        .body(Body::from("{}"))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(mismatched).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "GET", "/api/me", "", Some(&cookie))
            .await
            .status(),
        StatusCode::OK
    );
    let matching = Request::builder()
        .method("POST")
        .uri("/api/logout")
        .header("content-type", "application/json")
        .header("origin", "https://finanzas.example")
        .header("cookie", &cookie)
        .header("x-finapp-user", user_id)
        .body(Body::from("{}"))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(matching).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(&app, "GET", "/api/me", "", Some(&cookie))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn login_purges_expired_sessions() {
    use rusqlite::params;
    let db = Database::open_memory().unwrap();
    db.bootstrap_admin("alice", "password-1234").unwrap();
    let handle = db.clone();
    let app = app(db, "https://finanzas.example".to_owned());
    let first = request(
        &app,
        "POST",
        "/api/login",
        r#"{"username":"alice","password":"password-1234"}"#,
        None,
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let user_id: String = {
        let conn = handle.0.lock().unwrap();
        conn.query_row("SELECT id FROM users WHERE username='alice'", [], |r| {
            r.get(0)
        })
        .unwrap()
    };
    {
        let conn = handle.0.lock().unwrap();
        conn.execute(
            "INSERT INTO sessions(token_hash,user_id,expires_at) VALUES('expired',?1,1)",
            params![user_id],
        )
        .unwrap();
    }
    let second = request(
        &app,
        "POST",
        "/api/login",
        r#"{"username":"alice","password":"password-1234"}"#,
        None,
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);
    let expired: i64 = {
        let conn = handle.0.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE token_hash='expired'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(expired, 0);
}
