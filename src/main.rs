use std::{env, fmt::Write, time::Duration};

use askama::Template;
use axum::{
    extract::{rejection::FormRejection, Path, Query, Request, State},
    http::{
        header::{
            AUTHORIZATION, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE, WWW_AUTHENTICATE,
        },
        HeaderValue, Method, StatusCode,
    },
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Form, Router,
};
use base64::prelude::*;
use constant_time_eq::constant_time_eq;
use qrcodegen::{QrCode, QrCodeEcc};
use queueticket::{
    find_ticket, format_jst, guest_poll_interval, is_same_origin, issue_ticket, reset_tickets,
    transition_ticket, waiting_ahead, AppError, Ticket,
};
use serde::Deserialize;
use sqlx::{postgres::PgPoolOptions, PgPool};
use url::Url;
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    pool: PgPool,
    base_url: String,
    base: Url,
    expected_auth_header: String,
}

#[derive(Template)]
#[template(path = "admin_index.html")]
struct AdminIndexTemplate;

#[derive(Template)]
#[template(path = "front.html")]
struct FrontTemplate {
    last_ticket: Option<Ticket>,
    qr_code: Option<String>,
    request_id: Uuid,
    history: Vec<Ticket>,
}

#[derive(Template)]
#[template(path = "call.html")]
struct CallTemplate {
    tickets: Vec<Ticket>,
    conflict: bool,
}

#[derive(Template)]
#[template(path = "call_items.html")]
struct CallListTemplate {
    tickets: Vec<Ticket>,
}

#[derive(Template)]
#[template(path = "guest.html")]
struct GuestTemplate {
    ticket: Ticket,
    waiting_count: i64,
    interval: u32,
}

#[derive(Template)]
#[template(path = "guest_content.html")]
struct GuestContentTemplate {
    ticket: Ticket,
    waiting_count: i64,
}

fn render(template: impl Template) -> Result<Html<String>, AppError> {
    template
        .render()
        .map(Html)
        .map_err(|error| AppError::Internal(error.to_string()))
}

fn to_svg_string(qr: &QrCode, border: i32) -> String {
    let mut res = String::new();
    let dim = qr.size();
    let width = dim + border * 2;
    let _ = write!(res, "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {width} {width}\" stroke=\"none\">");
    res.push_str("<rect width=\"100%\" height=\"100%\" fill=\"#FFFFFF\"/><path d=\"");
    for y in 0..dim {
        for x in 0..dim {
            if qr.get_module(x, y) {
                let _ = write!(res, "M{},{}h1v1h-1z ", x + border, y + border);
            }
        }
    }
    res.push_str("\" fill=\"#000000\"/></svg>");
    res
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Err(error) = dotenvy::dotenv() {
        if !error.not_found() {
            return Err(error.into());
        }
    }
    let database_url = env::var("DATABASE_URL")?;
    let admin_password = env::var("ADMIN_PASSWORD")?;
    if admin_password.is_empty() {
        return Err("ADMIN_PASSWORD must not be empty".into());
    }
    let base_url = env::var("BASE_URL")?.trim_end_matches('/').to_owned();
    let base = Url::parse(&base_url)?;
    if !matches!(base.scheme(), "http" | "https")
        || base.host_str().is_none()
        || base.query().is_some()
        || base.fragment().is_some()
        || !base.username().is_empty()
        || base.password().is_some()
    {
        return Err(
            "BASE_URL must be an HTTP(S) URL without credentials, query, or fragment".into(),
        );
    }
    let port: u16 = env::var("PORT")
        .unwrap_or_else(|_| "8000".to_owned())
        .parse()?;
    let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0".to_owned());
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&database_url)
        .await?;
    sqlx::migrate!().run(&pool).await?;
    let state = AppState {
        pool: pool.clone(),
        base_url,
        base,
        expected_auth_header: format!(
            "Basic {}",
            BASE64_STANDARD.encode(format!("admin:{admin_password}"))
        ),
    };
    let app = router(state);
    let listener = tokio::net::TcpListener::bind((bind_addr.as_str(), port)).await?;
    eprintln!("Listening on {}", listener.local_addr()?);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    pool.close().await;
    Ok(())
}

fn router(state: AppState) -> Router {
    let admin_routes = Router::new()
        .route("/admin", get(admin_index))
        .route("/admin/reset", post(reset_db))
        .route("/admin/download_csv", get(download_csv))
        .route("/admin/front", get(front_page))
        .route("/admin/front/tickets", post(create_ticket))
        .route("/admin/front/tickets/{id}", get(front_ticket))
        .route("/admin/call", get(call_page))
        .route("/admin/call/list", get(call_list))
        .route("/admin/call/update", post(update_status))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth));
    Router::new()
        .merge(admin_routes)
        .route("/", get(|| async { Redirect::to("/admin") }))
        .route("/guest/{id}", get(guest_page))
        .route("/guest/{id}/content", get(guest_content))
        .layer(middleware::from_fn(no_store))
        .route("/static/poll.js", get(poll_script))
        .with_state(state)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("Ctrl+C signal error: {error}");
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                eprintln!("SIGTERM signal error: {error}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { () = ctrl_c => {}, () = terminate => {} }
}

async fn no_store(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn poll_script() -> impl IntoResponse {
    (
        [
            (CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../static/poll.js"),
    )
}

async fn auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let authorized = req.headers().get(AUTHORIZATION).is_some_and(|value| {
        constant_time_eq(value.as_bytes(), state.expected_auth_header.as_bytes())
    });
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            [(WWW_AUTHENTICATE, "Basic realm=\"Admin Area\"")],
            "認証が必要です",
        )
            .into_response();
    }
    if !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        // An invalid/present Origin must not fall back to a valid Referer.
        let source = req
            .headers()
            .get("origin")
            .or_else(|| req.headers().get("referer"));
        let valid = source
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| is_same_origin(value, &state.base));
        if !valid {
            return (
                StatusCode::FORBIDDEN,
                Html("送信元を確認できません。管理画面から操作してください"),
            )
                .into_response();
        }
    }
    next.run(req).await
}

async fn reset_db(State(state): State<AppState>) -> Result<Redirect, AppError> {
    reset_tickets(&state.pool).await?;
    Ok(Redirect::to("/admin"))
}

async fn admin_index() -> Result<Html<String>, AppError> {
    render(AdminIndexTemplate)
}

async fn front_template(
    state: &AppState,
    last_ticket: Option<Ticket>,
) -> Result<Html<String>, AppError> {
    let qr_code = match &last_ticket {
        Some(ticket) => {
            let url = format!("{}/guest/{}", state.base_url, ticket.id);
            let qr = QrCode::encode_text(&url, QrCodeEcc::Medium)
                .map_err(|error| AppError::Internal(format!("QR error: {error:?}")))?;
            Some(to_svg_string(&qr, 4))
        }
        None => None,
    };
    let history = sqlx::query_as("SELECT * FROM tickets ORDER BY seq DESC LIMIT 20")
        .fetch_all(&state.pool)
        .await?;
    render(FrontTemplate {
        last_ticket,
        qr_code,
        request_id: Uuid::new_v4(),
        history,
    })
}

async fn front_page(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    front_template(&state, None).await
}

async fn front_ticket(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Html<String>, AppError> {
    let ticket = find_ticket(&state.pool, id).await?;
    front_template(&state, Some(ticket)).await
}

#[derive(Deserialize)]
struct CreateTicketForm {
    group_size: i32,
    request_id: Uuid,
}

async fn create_ticket(
    State(state): State<AppState>,
    form: Result<Form<CreateTicketForm>, FormRejection>,
) -> Result<Redirect, AppError> {
    let Form(form) = form.map_err(|_| {
        AppError::Validation("人数は1〜20名で入力してください。発券画面から送信し直してください")
    })?;
    let ticket = issue_ticket(&state.pool, form.group_size, form.request_id).await?;
    Ok(Redirect::to(&format!("/admin/front/tickets/{}", ticket.id)))
}

async fn active_tickets(pool: &PgPool) -> Result<Vec<Ticket>, AppError> {
    Ok(
        sqlx::query_as("SELECT * FROM tickets WHERE status <> 'completed' ORDER BY seq")
            .fetch_all(pool)
            .await?,
    )
}

#[derive(Deserialize)]
struct CallQuery {
    conflict: Option<String>,
}

async fn call_page(
    State(state): State<AppState>,
    Query(query): Query<CallQuery>,
) -> Result<Html<String>, AppError> {
    render(CallTemplate {
        tickets: active_tickets(&state.pool).await?,
        conflict: query.conflict.as_deref() == Some("1"),
    })
}

async fn call_list(State(state): State<AppState>) -> Result<Response, AppError> {
    Ok((
        [("X-Poll-Interval", "10")],
        render(CallListTemplate {
            tickets: active_tickets(&state.pool).await?,
        })?,
    )
        .into_response())
}

#[derive(Deserialize)]
struct UpdateStatusForm {
    id: Uuid,
    from: String,
    status: String,
}

async fn update_status(
    State(state): State<AppState>,
    form: Result<Form<UpdateStatusForm>, FormRejection>,
) -> Result<Redirect, AppError> {
    let Form(form) = form.map_err(|_| AppError::Validation("状態変更の入力が不正です"))?;
    let affected = transition_ticket(&state.pool, form.id, &form.from, &form.status).await?;
    Ok(Redirect::to(if affected == 0 {
        "/admin/call?conflict=1"
    } else {
        "/admin/call"
    }))
}

async fn guest_page(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Html<String>, AppError> {
    let ticket = find_ticket(&state.pool, id).await?;
    let waiting_count = waiting_ahead(&state.pool, ticket.seq).await?;
    let interval = guest_poll_interval(&ticket.status, waiting_count);
    render(GuestTemplate {
        ticket,
        waiting_count,
        interval,
    })
}

async fn guest_content(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Response, AppError> {
    let ticket = find_ticket(&state.pool, id).await?;
    let waiting_count = waiting_ahead(&state.pool, ticket.seq).await?;
    let interval = guest_poll_interval(&ticket.status, waiting_count);
    Ok((
        [("X-Poll-Interval", interval.to_string())],
        render(GuestContentTemplate {
            ticket,
            waiting_count,
        })?,
    )
        .into_response())
}

async fn download_csv(State(state): State<AppState>) -> Result<Response, AppError> {
    let tickets: Vec<Ticket> = sqlx::query_as("SELECT * FROM tickets ORDER BY seq")
        .fetch_all(&state.pool)
        .await?;
    let mut csv = String::from(
        "内部通番seq,整理番号,人数,ステータス,発券時刻(JST),呼出時刻(JST),完了時刻(JST)\n",
    );
    for ticket in tickets {
        let called = ticket.called_at.map(format_jst).unwrap_or_default();
        let completed = ticket.completed_at.map(format_jst).unwrap_or_default();
        let _ = writeln!(
            csv,
            "{},{},{},{},{},{},{}",
            ticket.seq,
            ticket.number,
            ticket.group_size,
            ticket.status,
            format_jst(ticket.created_at),
            called,
            completed
        );
    }
    Ok((
        [
            (CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                CONTENT_DISPOSITION,
                "attachment; filename=\"tickets_log.csv\"",
            ),
        ],
        csv,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt;

    async fn test_app() -> Router {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://localhost/unused")
            .unwrap();
        // A closed pool produces DB errors immediately and never accesses a network.
        pool.close().await;
        router(AppState {
            pool,
            base_url: "https://example.com".to_owned(),
            base: Url::parse("https://example.com").unwrap(),
            expected_auth_header: "Basic test".to_owned(),
        })
    }

    fn post_request(path: &str) -> axum::http::request::Builder {
        Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(AUTHORIZATION, "Basic test")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
    }

    #[tokio::test]
    async fn form_validation_returns_japanese_bad_request_without_db() {
        let app = test_app().await;
        for size in ["0", "21", "abc", "9999999999999"] {
            let response = app
                .clone()
                .oneshot(
                    post_request("/admin/front/tickets")
                        .header("Origin", "https://example.com")
                        .body(Body::from(format!(
                            "group_size={size}&request_id={}",
                            Uuid::new_v4()
                        )))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            assert!(String::from_utf8(body.to_vec())
                .unwrap()
                .contains("人数は1〜20名"));
        }
        let response = app
            .oneshot(
                post_request("/admin/call/update")
                    .header("Origin", "https://example.com")
                    .body(Body::from(format!(
                        "id={}&from=waiting&status=completed",
                        Uuid::new_v4()
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn csrf_rejects_invalid_origin_and_uses_referer_only_if_origin_absent() {
        let app = test_app().await;
        for origin in [
            None,
            Some("null"),
            Some("https://example.com.evil.com"),
            Some("http://example.com"),
            Some("https://example.com:444"),
        ] {
            let mut request = post_request("/admin/reset");
            if let Some(origin) = origin {
                request = request
                    .header("Origin", origin)
                    .header("Referer", "https://example.com/admin");
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        }
        // Missing Origin + matching Referer passes CSRF, then reaches the closed DB.
        let response = app
            .oneshot(
                post_request("/admin/reset")
                    .header("Referer", "https://example.com/admin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn cache_auth_and_error_responses() {
        let app = test_app().await;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/static/poll.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CACHE_CONTROL], "public, max-age=3600");
        for (path, status) in [
            ("/admin", StatusCode::UNAUTHORIZED),
            ("/missing", StatusCode::NOT_FOUND),
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        }
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/guest/{}", Uuid::new_v4()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8(body.to_vec())
            .unwrap()
            .contains("サーバーに接続できません。しばらくして再読み込みしてください"));
    }
}
