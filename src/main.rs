use std::{
    env,
    fmt::Write,
    sync::{Arc, Mutex},
    time::Duration,
};

use askama::Template;
use axum::{
    extract::{rejection::FormRejection, Path, Query, Request, State},
    http::{
        header::{
            AUTHORIZATION, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE, WWW_AUTHENTICATE,
        },
        HeaderMap, HeaderValue, Method, StatusCode,
    },
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Form, Json, Router,
};
use base64::prelude::*;
use chrono::{DateTime, TimeDelta, Utc};
use constant_time_eq::constant_time_eq;
use qrcodegen::{QrCode, QrCodeEcc};
use queueticket::{
    call_next, call_phase, estimate_wait_minutes, find_ticket, format_jst, format_jst_hm,
    front_phase, guest_poll_interval, is_same_origin, issue_ticket, mark_first_viewed,
    mark_returned, minutes_since, recent_call_count, reset_tickets, seat_partial,
    transition_ticket, waiting_ahead, AppError, CallPhase, ReturnOutcome, SeatOutcome, Ticket,
    WAIT_ESTIMATE_WINDOW_MINUTES,
};
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgPoolOptions, PgPool};
use url::Url;
use uuid::Uuid;

#[derive(Clone, Copy, Debug)]
struct FrontDisplay {
    ticket_id: Uuid,
    shown_at: DateTime<Utc>,
}

#[derive(Clone)]
struct AppState {
    pool: PgPool,
    base_url: String,
    base: Url,
    expected_auth_header: String,
    front_display: Arc<Mutex<Option<FrontDisplay>>>,
}

#[derive(Template)]
#[template(path = "admin_index.html")]
struct AdminIndexTemplate;

#[derive(Template)]
#[template(path = "display_front.html")]
struct DisplayFrontTemplate;

#[derive(Template)]
#[template(path = "display_screen.html")]
struct DisplayScreenTemplate;

#[derive(Template)]
#[template(path = "front.html")]
struct FrontTemplate {
    last_ticket: Option<Ticket>,
    qr_code: Option<String>,
    request_id: Uuid,
    history: Vec<Ticket>,
}

struct ReturnedTicketView {
    ticket: Ticket,
    returned_hm: String,
    remaining: i32,
}

struct CalledTicketView {
    ticket: Ticket,
    elapsed_minutes: i64,
    remaining: i32,
}

struct SeatedNotice {
    number: i32,
    seat: i32,
    remaining: i32,
}

struct CallGroups {
    returned_tickets: Vec<ReturnedTicketView>,
    calling_tickets: Vec<CalledTicketView>,
    absent_tickets: Vec<CalledTicketView>,
    waiting_tickets: Vec<Ticket>,
}

#[derive(Template)]
#[template(path = "call.html")]
struct CallTemplate {
    returned_tickets: Vec<ReturnedTicketView>,
    calling_tickets: Vec<CalledTicketView>,
    absent_tickets: Vec<CalledTicketView>,
    waiting_tickets: Vec<Ticket>,
    request_id: Uuid,
    called: Option<i32>,
    seated_notice: Option<SeatedNotice>,
    completed_number: Option<i32>,
    empty: bool,
    conflict: bool,
}

#[derive(Template)]
#[template(path = "call_items.html")]
struct CallListTemplate {
    returned_tickets: Vec<ReturnedTicketView>,
    calling_tickets: Vec<CalledTicketView>,
    absent_tickets: Vec<CalledTicketView>,
    waiting_tickets: Vec<Ticket>,
}

struct WaitEstimateView {
    minutes: i64,
    around_hm: String,
}

#[derive(Template)]
#[template(path = "guest.html")]
struct GuestTemplate {
    ticket: Ticket,
    waiting_count: i64,
    interval: u32,
    phase: Option<&'static str>,
    wait_estimate: Option<WaitEstimateView>,
}

#[derive(Template)]
#[template(path = "guest_content.html")]
struct GuestContentTemplate {
    ticket: Ticket,
    waiting_count: i64,
    phase: Option<&'static str>,
    wait_estimate: Option<WaitEstimateView>,
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

fn guest_qr_svg(base_url: &str, ticket_id: Uuid) -> Result<String, AppError> {
    let url = format!("{base_url}/guest/{ticket_id}");
    let qr = QrCode::encode_text(&url, QrCodeEcc::Medium)
        .map_err(|error| AppError::Internal(format!("QR error: {error:?}")))?;
    Ok(to_svg_string(&qr, 4))
}

fn set_front_display(state: &AppState, display: Option<FrontDisplay>) {
    let mut guard = state
        .front_display
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = display;
}

fn get_front_display(state: &AppState) -> Option<FrontDisplay> {
    *state
        .front_display
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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
        front_display: Arc::new(Mutex::new(None)),
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
        .route("/admin/display/front", get(display_front_page))
        .route("/admin/display/front/clear", post(clear_front_display))
        .route("/admin/display/screen", get(display_screen_page))
        .route("/admin/display/state", get(display_state))
        .route("/admin/call", get(call_page))
        .route("/admin/call/list", get(call_list))
        .route("/admin/call/next", post(call_next_ticket))
        .route("/admin/call/seat", post(seat_partial_ticket))
        .route("/admin/call/return", post(admin_return_ticket))
        .route("/admin/call/update", post(update_status))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth));
    Router::new()
        .merge(admin_routes)
        .route("/", get(|| async { Redirect::to("/admin") }))
        .route("/guest/{id}", get(guest_page))
        .route("/guest/{id}/content", get(guest_content))
        .route("/guest/{id}/return", post(guest_return))
        .layer(middleware::from_fn(no_store))
        .route("/static/poll.js", get(poll_script))
        .route("/static/display.js", get(display_script))
        .route("/static/call.js", get(call_script))
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

async fn display_script() -> impl IntoResponse {
    (
        [
            (CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../static/display.js"),
    )
}

async fn call_script() -> impl IntoResponse {
    (
        [
            (CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../static/call.js"),
    )
}

fn has_same_origin(headers: &HeaderMap, base: &Url) -> bool {
    // An invalid/present Origin must not fall back to a valid Referer.
    let source = headers.get("origin").or_else(|| headers.get("referer"));
    source
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| is_same_origin(value, base))
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
    if !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS)
        && !has_same_origin(req.headers(), &state.base)
    {
        return (
            StatusCode::FORBIDDEN,
            Html("送信元を確認できません。管理画面から操作してください"),
        )
            .into_response();
    }
    next.run(req).await
}

async fn reset_db(State(state): State<AppState>) -> Result<Redirect, AppError> {
    reset_tickets(&state.pool).await?;
    set_front_display(&state, None);
    Ok(Redirect::to("/admin"))
}

async fn admin_index() -> Result<Html<String>, AppError> {
    render(AdminIndexTemplate)
}

async fn display_front_page() -> Result<Html<String>, AppError> {
    render(DisplayFrontTemplate)
}

async fn display_screen_page() -> Result<Html<String>, AppError> {
    render(DisplayScreenTemplate)
}

async fn clear_front_display(State(state): State<AppState>) -> Redirect {
    set_front_display(&state, None);
    Redirect::to("/admin/front")
}

#[derive(Serialize)]
struct DisplayTicketItem {
    seq: i64,
    number: i32,
    group_size: i32,
    remaining: i32,
}

#[derive(Serialize)]
struct DisplayFrontState {
    seq: i64,
    number: i32,
    group_size: i32,
    qr_svg: String,
    phase: &'static str,
}

#[derive(Serialize)]
struct DisplayStateResponse {
    clock: String,
    calling: Vec<DisplayTicketItem>,
    absent: Vec<DisplayTicketItem>,
    upcoming: Vec<DisplayTicketItem>,
    waiting_count: usize,
    front: Option<DisplayFrontState>,
}

async fn display_state(
    State(state): State<AppState>,
) -> Result<Json<DisplayStateResponse>, AppError> {
    let tickets: Vec<Ticket> =
        sqlx::query_as("SELECT * FROM tickets WHERE status <> 'completed' ORDER BY seq")
            .fetch_all(&state.pool)
            .await?;
    let now = Utc::now();
    let mut calling = Vec::new();
    let mut absent = Vec::new();
    let mut upcoming = Vec::new();
    let mut waiting_count = 0usize;

    for ticket in tickets {
        match call_phase(&ticket, now) {
            Some(CallPhase::Calling | CallPhase::Returned) => {
                calling.push(DisplayTicketItem {
                    seq: ticket.seq,
                    number: ticket.number,
                    group_size: ticket.group_size,
                    remaining: ticket.remaining(),
                });
            }
            Some(CallPhase::Absent) => {
                absent.push(DisplayTicketItem {
                    seq: ticket.seq,
                    number: ticket.number,
                    group_size: ticket.group_size,
                    remaining: ticket.remaining(),
                });
            }
            None => {
                waiting_count += 1;
                if upcoming.len() < 3 {
                    upcoming.push(DisplayTicketItem {
                        seq: ticket.seq,
                        number: ticket.number,
                        group_size: ticket.group_size,
                        remaining: ticket.remaining(),
                    });
                }
            }
        }
    }

    let front = if let Some(fd) = get_front_display(&state) {
        let front_ticket: Option<Ticket> = sqlx::query_as("SELECT * FROM tickets WHERE id=$1")
            .bind(fd.ticket_id)
            .fetch_optional(&state.pool)
            .await?;
        match front_ticket {
            Some(ticket) => match front_phase(fd.shown_at, ticket.first_viewed_at, now) {
                Some(phase) => {
                    let qr_svg = guest_qr_svg(&state.base_url, ticket.id)?;
                    Some(DisplayFrontState {
                        seq: ticket.seq,
                        number: ticket.number,
                        group_size: ticket.group_size,
                        qr_svg,
                        phase: phase.as_str(),
                    })
                }
                None => None,
            },
            None => None,
        }
    } else {
        None
    };

    Ok(Json(DisplayStateResponse {
        clock: format_jst_hm(now),
        calling,
        absent,
        upcoming,
        waiting_count,
        front,
    }))
}

async fn front_template(
    state: &AppState,
    last_ticket: Option<Ticket>,
) -> Result<Html<String>, AppError> {
    let qr_code = match &last_ticket {
        Some(ticket) => Some(guest_qr_svg(&state.base_url, ticket.id)?),
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
    set_front_display(
        &state,
        Some(FrontDisplay {
            ticket_id: id,
            shown_at: Utc::now(),
        }),
    );
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

async fn load_call_groups(pool: &PgPool) -> Result<CallGroups, AppError> {
    let tickets: Vec<Ticket> =
        sqlx::query_as("SELECT * FROM tickets WHERE status <> 'completed' ORDER BY seq")
            .fetch_all(pool)
            .await?;
    let now = Utc::now();
    let mut returned_tickets = Vec::new();
    let mut calling_tickets = Vec::new();
    let mut absent_tickets = Vec::new();
    let mut waiting_tickets = Vec::new();
    for ticket in tickets {
        match call_phase(&ticket, now) {
            Some(CallPhase::Returned) => {
                let returned_hm = ticket.returned_at.map(format_jst_hm).unwrap_or_default();
                let remaining = ticket.remaining();
                returned_tickets.push(ReturnedTicketView {
                    ticket,
                    returned_hm,
                    remaining,
                });
            }
            Some(CallPhase::Calling) => {
                let elapsed_minutes = ticket
                    .called_at
                    .map(|called_at| minutes_since(called_at, now))
                    .unwrap_or(0);
                let remaining = ticket.remaining();
                calling_tickets.push(CalledTicketView {
                    ticket,
                    elapsed_minutes,
                    remaining,
                });
            }
            Some(CallPhase::Absent) => {
                let elapsed_minutes = ticket
                    .called_at
                    .map(|called_at| minutes_since(called_at, now))
                    .unwrap_or(0);
                let remaining = ticket.remaining();
                absent_tickets.push(CalledTicketView {
                    ticket,
                    elapsed_minutes,
                    remaining,
                });
            }
            None => waiting_tickets.push(ticket),
        }
    }
    Ok(CallGroups {
        returned_tickets,
        calling_tickets,
        absent_tickets,
        waiting_tickets,
    })
}

#[derive(Deserialize)]
struct CallQuery {
    conflict: Option<String>,
    called: Option<i32>,
    seated: Option<i32>,
    number: Option<i32>,
    remaining: Option<i32>,
    completed: Option<i32>,
    empty: Option<String>,
}

async fn call_page(
    State(state): State<AppState>,
    Query(query): Query<CallQuery>,
) -> Result<Html<String>, AppError> {
    let groups = load_call_groups(&state.pool).await?;
    let seated_notice = match (query.seated, query.number, query.remaining) {
        (Some(seat), Some(number), Some(remaining)) => Some(SeatedNotice {
            number,
            seat,
            remaining,
        }),
        _ => None,
    };
    render(CallTemplate {
        returned_tickets: groups.returned_tickets,
        calling_tickets: groups.calling_tickets,
        absent_tickets: groups.absent_tickets,
        waiting_tickets: groups.waiting_tickets,
        request_id: Uuid::new_v4(),
        called: query.called,
        seated_notice,
        completed_number: query.completed,
        empty: query.empty.as_deref() == Some("1"),
        conflict: query.conflict.as_deref() == Some("1"),
    })
}

async fn call_list(State(state): State<AppState>) -> Result<Response, AppError> {
    let groups = load_call_groups(&state.pool).await?;
    Ok((
        [("X-Poll-Interval", "10")],
        render(CallListTemplate {
            returned_tickets: groups.returned_tickets,
            calling_tickets: groups.calling_tickets,
            absent_tickets: groups.absent_tickets,
            waiting_tickets: groups.waiting_tickets,
        })?,
    )
        .into_response())
}

#[derive(Deserialize)]
struct CallNextForm {
    request_id: Uuid,
}

async fn call_next_ticket(
    State(state): State<AppState>,
    form: Result<Form<CallNextForm>, FormRejection>,
) -> Result<Redirect, AppError> {
    let Form(form) = form.map_err(|_| {
        AppError::Validation("呼び出しリクエストが不正です。画面を再読み込みしてください")
    })?;
    match call_next(&state.pool, form.request_id).await? {
        Some(ticket) => Ok(Redirect::to(&format!(
            "/admin/call?called={}",
            ticket.number
        ))),
        None => Ok(Redirect::to("/admin/call?empty=1")),
    }
}

#[derive(Deserialize)]
struct AdminReturnForm {
    id: Uuid,
}

async fn admin_return_ticket(
    State(state): State<AppState>,
    form: Result<Form<AdminReturnForm>, FormRejection>,
) -> Result<Redirect, AppError> {
    let Form(form) = form.map_err(|_| AppError::Validation("戻り確認の入力が不正です"))?;
    match mark_returned(&state.pool, form.id, Utc::now(), false).await? {
        ReturnOutcome::Returned | ReturnOutcome::AlreadyReturned => Ok(Redirect::to("/admin/call")),
        ReturnOutcome::NotEligible => Ok(Redirect::to("/admin/call?conflict=1")),
    }
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

#[derive(Deserialize)]
struct SeatForm {
    id: Uuid,
    expected_seated: i32,
    seat: i32,
}

async fn seat_partial_ticket(
    State(state): State<AppState>,
    form: Result<Form<SeatForm>, FormRejection>,
) -> Result<Redirect, AppError> {
    let Form(form) = form.map_err(|_| {
        AppError::Validation("一部案内の入力が不正です。画面を再読み込みしてください")
    })?;
    let number = find_ticket(&state.pool, form.id).await?.number;
    match seat_partial(&state.pool, form.id, form.seat, form.expected_seated).await? {
        SeatOutcome::Partial { remaining } => Ok(Redirect::to(&format!(
            "/admin/call?seated={}&number={}&remaining={}",
            form.seat, number, remaining
        ))),
        SeatOutcome::Completed => Ok(Redirect::to(&format!("/admin/call?completed={}", number))),
        SeatOutcome::Conflict => Ok(Redirect::to("/admin/call?conflict=1")),
    }
}

struct GuestView {
    ticket: Ticket,
    waiting_count: i64,
    interval: u32,
    phase: Option<&'static str>,
    wait_estimate: Option<WaitEstimateView>,
}

async fn load_guest_view(pool: &PgPool, id: Uuid) -> Result<GuestView, AppError> {
    let ticket = find_ticket(pool, id).await?;
    let waiting_count = waiting_ahead(pool, ticket.seq).await?;
    let interval = guest_poll_interval(&ticket.status, waiting_count);
    let now = Utc::now();
    let phase = call_phase(&ticket, now).map(CallPhase::as_str);
    let wait_estimate = if ticket.status == "waiting" {
        let recent_calls = recent_call_count(pool, WAIT_ESTIMATE_WINDOW_MINUTES).await?;
        estimate_wait_minutes(waiting_count, recent_calls, WAIT_ESTIMATE_WINDOW_MINUTES).map(
            |minutes| WaitEstimateView {
                minutes,
                around_hm: format_jst_hm(now + TimeDelta::minutes(minutes)),
            },
        )
    } else {
        None
    };
    Ok(GuestView {
        ticket,
        waiting_count,
        interval,
        phase,
        wait_estimate,
    })
}

async fn guest_page(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Html<String>, AppError> {
    mark_first_viewed(&state.pool, id).await?;
    let view = load_guest_view(&state.pool, id).await?;
    render(GuestTemplate {
        ticket: view.ticket,
        waiting_count: view.waiting_count,
        interval: view.interval,
        phase: view.phase,
        wait_estimate: view.wait_estimate,
    })
}

async fn guest_content(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Response, AppError> {
    let view = load_guest_view(&state.pool, id).await?;
    Ok((
        [("X-Poll-Interval", view.interval.to_string())],
        render(GuestContentTemplate {
            ticket: view.ticket,
            waiting_count: view.waiting_count,
            phase: view.phase,
            wait_estimate: view.wait_estimate,
        })?,
    )
        .into_response())
}

async fn guest_return(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if !has_same_origin(&headers, &state.base) {
        return Ok((
            StatusCode::FORBIDDEN,
            Html("送信元を確認できません。画面を再読み込みして操作してください"),
        )
            .into_response());
    }
    let outcome = mark_returned(&state.pool, id, Utc::now(), true).await?;
    let is_fetch = headers
        .get("x-requested-with")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("fetch"));
    if is_fetch {
        let status = match outcome {
            ReturnOutcome::Returned | ReturnOutcome::AlreadyReturned => StatusCode::NO_CONTENT,
            ReturnOutcome::NotEligible => StatusCode::CONFLICT,
        };
        Ok(status.into_response())
    } else {
        Ok(Redirect::to(&format!("/guest/{id}")).into_response())
    }
}

async fn download_csv(State(state): State<AppState>) -> Result<Response, AppError> {
    let tickets: Vec<Ticket> = sqlx::query_as("SELECT * FROM tickets ORDER BY seq")
        .fetch_all(&state.pool)
        .await?;
    let mut csv = String::from(
        "内部通番seq,整理番号,人数,ステータス,発券時刻(JST),呼出時刻(JST),完了時刻(JST),戻り申告時刻(JST),初回表示時刻(JST),案内済み人数\n",
    );
    for ticket in tickets {
        let called = ticket.called_at.map(format_jst).unwrap_or_default();
        let completed = ticket.completed_at.map(format_jst).unwrap_or_default();
        let returned = ticket.returned_at.map(format_jst).unwrap_or_default();
        let first_viewed = ticket.first_viewed_at.map(format_jst).unwrap_or_default();
        let _ = writeln!(
            csv,
            "{},{},{},{},{},{},{},{},{},{}",
            ticket.seq,
            ticket.number,
            ticket.group_size,
            ticket.status,
            format_jst(ticket.created_at),
            called,
            completed,
            returned,
            first_viewed,
            ticket.seated_count
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
            front_display: Arc::new(Mutex::new(None)),
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

    #[tokio::test]
    async fn guest_return_and_call_next_enforce_csrf_and_auth() {
        let app = test_app().await;
        let guest_return_path = format!("/guest/{}/return", Uuid::new_v4());
        for origin in [None, Some("https://evil.example.com"), Some("null")] {
            let mut req = Request::builder()
                .method(Method::POST)
                .uri(&guest_return_path);
            if let Some(origin) = origin {
                req = req.header("Origin", origin);
            }
            let response = app
                .clone()
                .oneshot(req.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        }
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&guest_return_path)
                    .header("Origin", "https://example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/admin/call/next")
                    .header("Origin", "https://example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .clone()
            .oneshot(
                post_request("/admin/call/next")
                    .header("Origin", "https://evil.example.com")
                    .body(Body::from(format!("request_id={}", Uuid::new_v4())))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let response = app
            .oneshot(
                post_request("/admin/call/next")
                    .header("Origin", "https://example.com")
                    .body(Body::from("request_id=invalid"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn seat_partial_route_enforces_auth_csrf_and_validation() {
        let app = test_app().await;

        // No credentials -> 401
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/admin/call/seat")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");

        // Valid credentials but invalid Origin -> 403
        let response = app
            .clone()
            .oneshot(
                post_request("/admin/call/seat")
                    .header("Origin", "https://evil.example.com")
                    .body(Body::from(format!(
                        "id={}&expected_seated=0&seat=1",
                        Uuid::new_v4()
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");

        // Valid credentials and Origin, malformed form -> 400
        let response = app
            .clone()
            .oneshot(
                post_request("/admin/call/seat")
                    .header("Origin", "https://example.com")
                    .body(Body::from(format!(
                        "id={}&expected_seated=0&seat=abc",
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
            .contains("一部案内の入力が不正です"));
    }

    #[tokio::test]
    async fn call_script_is_public_and_cached() {
        let app = test_app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/static/call.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CACHE_CONTROL], "public, max-age=3600");
        assert_eq!(
            response.headers()[CONTENT_TYPE],
            "text/javascript; charset=utf-8"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8(body.to_vec())
            .unwrap()
            .contains("partial-open"));
    }

    #[tokio::test]
    async fn display_routes_enforce_auth_cache_and_csrf() {
        let app = test_app().await;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/static/display.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CACHE_CONTROL], "public, max-age=3600");

        for path in [
            "/admin/display/state",
            "/admin/display/front",
            "/admin/display/screen",
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        }

        for path in ["/admin/display/front", "/admin/display/screen"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(AUTHORIZATION, "Basic test")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        }

        let response = app
            .clone()
            .oneshot(
                post_request("/admin/display/front/clear")
                    .header("Origin", "https://evil.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");

        let response = app
            .oneshot(
                post_request("/admin/display/front/clear")
                    .header("Origin", "https://example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()["location"], "/admin/front");
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
    }
}
