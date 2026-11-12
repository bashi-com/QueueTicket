use std::collections::HashSet;

use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use chrono::{DateTime, FixedOffset, TimeDelta, Utc};
use sqlx::{FromRow, PgPool};
use url::Url;
use uuid::Uuid;

pub const MAX_GROUP_SIZE: i32 = 20;
pub const ABSENT_AFTER_MINUTES: i64 = 10;
pub const WAIT_ESTIMATE_WINDOW_MINUTES: i64 = 30;
pub const FRONT_SHOWING_SECONDS: i64 = 300;
pub const FRONT_VIEWED_SECONDS: i64 = 8;

#[derive(Debug)]
pub enum AppError {
    Database(sqlx::Error),
    NotFound,
    Validation(&'static str),
    Capacity,
    Internal(String),
}

impl From<sqlx::Error> for AppError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Database(error) => {
                eprintln!("Database error: {error:?}");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "サーバーに接続できません。しばらくして再読み込みしてください",
                )
            }
            Self::NotFound => (StatusCode::NOT_FOUND, "整理券が見つかりません"),
            Self::Validation(message) => (StatusCode::BAD_REQUEST, message),
            Self::Capacity => (
                StatusCode::SERVICE_UNAVAILABLE,
                "発券上限です。未完了の整理券が999件あります。しばらくお待ちください",
            ),
            Self::Internal(error) => {
                eprintln!("Application error: {error}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "画面を表示できません。しばらくして再読み込みしてください",
                )
            }
        };
        (status, [("cache-control", "no-store")], Html(format!(
            "<!DOCTYPE html><html lang=\"ja\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>エラー</title><body style=\"font-family:sans-serif;padding:2rem\"><h1>エラー</h1><p>{message}</p></body></html>"
        ))).into_response()
    }
}

#[derive(Debug, FromRow, Clone)]
pub struct Ticket {
    pub id: Uuid,
    pub seq: i64,
    pub number: i32,
    pub group_size: i32,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub called_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub idempotency_key: Option<Uuid>,
    pub returned_at: Option<DateTime<Utc>>,
    pub first_viewed_at: Option<DateTime<Utc>>,
    pub call_request_id: Option<Uuid>,
    pub seated_count: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallPhase {
    Calling,
    Absent,
    Returned,
}

impl CallPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Calling => "calling",
            Self::Absent => "absent",
            Self::Returned => "returned",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontPhase {
    Showing,
    Viewed,
}

impl FrontPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Showing => "showing",
            Self::Viewed => "viewed",
        }
    }
}

pub fn front_phase(
    shown_at: DateTime<Utc>,
    first_viewed_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<FrontPhase> {
    if now.signed_duration_since(shown_at) > TimeDelta::seconds(FRONT_SHOWING_SECONDS) {
        return None;
    }
    if let Some(viewed_at) = first_viewed_at {
        if viewed_at >= shown_at {
            if now.signed_duration_since(viewed_at) > TimeDelta::seconds(FRONT_VIEWED_SECONDS) {
                return None;
            }
            return Some(FrontPhase::Viewed);
        }
    }
    Some(FrontPhase::Showing)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnOutcome {
    Returned,
    AlreadyReturned,
    NotEligible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeatOutcome {
    Partial { remaining: i32 },
    Completed,
    Conflict,
}

impl Ticket {
    pub fn created_jst(&self) -> String {
        format_jst(self.created_at)
    }
    pub fn status_label(&self) -> &str {
        match self.status.as_str() {
            "waiting" => "待機中",
            "called" => "呼出中",
            "completed" => "完了",
            _ => "不明",
        }
    }
    pub fn remaining(&self) -> i32 {
        self.group_size - self.seated_count
    }
}

pub fn call_phase(ticket: &Ticket, now: DateTime<Utc>) -> Option<CallPhase> {
    if ticket.status != "called" {
        return None;
    }
    if ticket.returned_at.is_some() || ticket.seated_count > 0 {
        return Some(CallPhase::Returned);
    }
    if let Some(called_at) = ticket.called_at {
        if now.signed_duration_since(called_at) >= TimeDelta::minutes(ABSENT_AFTER_MINUTES) {
            return Some(CallPhase::Absent);
        }
    }
    Some(CallPhase::Calling)
}

pub fn minutes_since(t: DateTime<Utc>, now: DateTime<Utc>) -> i64 {
    now.signed_duration_since(t).num_minutes().max(0)
}

pub fn format_jst(date: DateTime<Utc>) -> String {
    // UTC+9 is always a valid fixed offset; no host timezone is consulted.
    match FixedOffset::east_opt(9 * 3600) {
        Some(offset) => date
            .with_timezone(&offset)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        None => unreachable!("UTC+9 is a valid offset"),
    }
}

pub fn format_jst_hm(date: DateTime<Utc>) -> String {
    match FixedOffset::east_opt(9 * 3600) {
        Some(offset) => date.with_timezone(&offset).format("%H:%M").to_string(),
        None => unreachable!("UTC+9 is a valid offset"),
    }
}

pub fn estimate_wait_minutes(ahead: i64, recent_calls: i64, window_minutes: i64) -> Option<i64> {
    if recent_calls < 3 || window_minutes <= 0 {
        return None;
    }
    let groups = ahead.max(0) + 1;
    let raw_minutes = (groups * window_minutes + recent_calls - 1) / recent_calls;
    let rounded = ((raw_minutes + 4) / 5) * 5;
    Some(rounded.max(5))
}

pub fn next_display_number(last: i32, active: &HashSet<i32>) -> Option<i32> {
    (1..=999)
        .map(|step| (last.rem_euclid(999) + step - 1) % 999 + 1)
        .find(|number| !active.contains(number))
}

pub fn validate_group_size(size: i32) -> Result<(), AppError> {
    if (1..=MAX_GROUP_SIZE).contains(&size) {
        Ok(())
    } else {
        Err(AppError::Validation("人数は1〜20名で入力してください"))
    }
}

pub fn is_same_origin(header: &str, base: &Url) -> bool {
    if header.trim() != header || header.chars().any(char::is_control) {
        return false;
    }
    match Url::parse(header) {
        Ok(url) => {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.origin() == base.origin()
        }
        Err(_) => false,
    }
}

pub fn guest_poll_interval(status: &str, waiting_ahead: i64) -> u32 {
    match status {
        "completed" => 0,
        "waiting" if waiting_ahead > 10 => 60,
        "waiting" if waiting_ahead >= 3 => 30,
        _ => 15,
    }
}

pub async fn issue_ticket(
    pool: &PgPool,
    group_size: i32,
    request_id: Uuid,
) -> Result<Ticket, AppError> {
    validate_group_size(group_size)?;
    let mut tx = pool.begin().await?;
    // Serialize issuing and resetting. Check idempotency AFTER acquiring this lock,
    // so concurrent retries see the first transaction's committed ticket.
    let last: i32 =
        sqlx::query_scalar("SELECT last_number FROM ticket_counter WHERE id=1 FOR UPDATE")
            .fetch_one(&mut *tx)
            .await?;
    if let Some(ticket) =
        sqlx::query_as::<_, Ticket>("SELECT * FROM tickets WHERE idempotency_key=$1")
            .bind(request_id)
            .fetch_optional(&mut *tx)
            .await?
    {
        tx.commit().await?;
        return Ok(ticket);
    }
    let active: HashSet<i32> =
        sqlx::query_scalar::<_, i32>("SELECT number FROM tickets WHERE status <> 'completed'")
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect();
    let number = next_display_number(last, &active).ok_or(AppError::Capacity)?;
    let ticket = sqlx::query_as::<_, Ticket>(
        "INSERT INTO tickets (number, group_size, idempotency_key) VALUES ($1,$2,$3) RETURNING *",
    )
    .bind(number)
    .bind(group_size)
    .bind(request_id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("UPDATE ticket_counter SET last_number=$1 WHERE id=1")
        .bind(number)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(ticket)
}

pub async fn transition_ticket(
    pool: &PgPool,
    id: Uuid,
    from: &str,
    to: &str,
) -> Result<u64, AppError> {
    let query = match (from, to) {
        ("waiting", "called") => {
            "UPDATE tickets SET status=$2, called_at=NOW() WHERE id=$1 AND status=$3"
        }
        ("called", "completed") => {
            "UPDATE tickets SET status=$2, completed_at=NOW(), seated_count=group_size WHERE id=$1 AND status=$3"
        }
        _ => return Err(AppError::Validation("不正な状態変更です")),
    };
    Ok(sqlx::query(query)
        .bind(id)
        .bind(to)
        .bind(from)
        .execute(pool)
        .await?
        .rows_affected())
}

pub async fn seat_partial(
    pool: &PgPool,
    id: Uuid,
    seat: i32,
    expected_seated: i32,
) -> Result<SeatOutcome, AppError> {
    if seat < 1 {
        return Err(AppError::Validation("案内人数が不正です"));
    }
    let updated = sqlx::query_as::<_, Ticket>(
        "UPDATE tickets \
         SET seated_count = seated_count + $2, \
             status = CASE WHEN seated_count + $2 = group_size THEN 'completed' ELSE status END, \
             completed_at = CASE WHEN seated_count + $2 = group_size THEN NOW() ELSE completed_at END \
         WHERE id = $1 \
           AND status = 'called' \
           AND seated_count = $3 \
           AND seated_count + $2 <= group_size \
         RETURNING *",
    )
    .bind(id)
    .bind(seat)
    .bind(expected_seated)
    .fetch_optional(pool)
    .await?;
    if let Some(ticket) = updated {
        if ticket.status == "completed" {
            return Ok(SeatOutcome::Completed);
        }
        return Ok(SeatOutcome::Partial {
            remaining: ticket.remaining(),
        });
    }
    find_ticket(pool, id).await?;
    Ok(SeatOutcome::Conflict)
}

pub async fn reset_tickets(pool: &PgPool) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT last_number FROM ticket_counter WHERE id=1 FOR UPDATE")
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("TRUNCATE TABLE tickets RESTART IDENTITY")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE ticket_counter SET last_number=0 WHERE id=1")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn find_ticket(pool: &PgPool, id: Uuid) -> Result<Ticket, AppError> {
    sqlx::query_as("SELECT * FROM tickets WHERE id=$1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
}

pub async fn waiting_ahead(pool: &PgPool, seq: i64) -> Result<i64, AppError> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM tickets WHERE status='waiting' AND seq < $1")
            .bind(seq)
            .fetch_one(pool)
            .await?,
    )
}

pub async fn mark_returned(
    pool: &PgPool,
    id: Uuid,
    now: DateTime<Utc>,
    require_absent: bool,
) -> Result<ReturnOutcome, AppError> {
    let cutoff = now - TimeDelta::minutes(ABSENT_AFTER_MINUTES);
    let affected = sqlx::query(
        "UPDATE tickets SET returned_at=$2 \
         WHERE id=$1 AND status='called' AND returned_at IS NULL \
         AND (NOT $3 OR called_at <= $4)",
    )
    .bind(id)
    .bind(now)
    .bind(require_absent)
    .bind(cutoff)
    .execute(pool)
    .await?
    .rows_affected();
    if affected > 0 {
        return Ok(ReturnOutcome::Returned);
    }
    let ticket = find_ticket(pool, id).await?;
    if ticket.status == "called" && ticket.returned_at.is_some() {
        Ok(ReturnOutcome::AlreadyReturned)
    } else {
        Ok(ReturnOutcome::NotEligible)
    }
}

pub async fn call_next(pool: &PgPool, request_id: Uuid) -> Result<Option<Ticket>, AppError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT last_number FROM ticket_counter WHERE id=1 FOR UPDATE")
        .fetch_one(&mut *tx)
        .await?;
    if let Some(ticket) =
        sqlx::query_as::<_, Ticket>("SELECT * FROM tickets WHERE call_request_id=$1")
            .bind(request_id)
            .fetch_optional(&mut *tx)
            .await?
    {
        tx.commit().await?;
        return Ok(Some(ticket));
    }
    for _ in 0..5 {
        let Some(id) = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM tickets WHERE status='waiting' ORDER BY seq LIMIT 1 FOR UPDATE SKIP LOCKED",
        )
        .fetch_optional(&mut *tx)
        .await?
        else {
            tx.commit().await?;
            return Ok(None);
        };
        if let Some(ticket) = sqlx::query_as::<_, Ticket>(
            "UPDATE tickets SET status='called', called_at=NOW(), call_request_id=$2 \
             WHERE id=$1 AND status='waiting' RETURNING *",
        )
        .bind(id)
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            tx.commit().await?;
            return Ok(Some(ticket));
        }
    }
    tx.commit().await?;
    Ok(None)
}

pub async fn mark_first_viewed(pool: &PgPool, id: Uuid) -> Result<(), AppError> {
    sqlx::query("UPDATE tickets SET first_viewed_at=NOW() WHERE id=$1 AND first_viewed_at IS NULL")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn recent_call_count(pool: &PgPool, window_minutes: i64) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM tickets WHERE called_at >= NOW() - ($1::float8 * INTERVAL '1 minute')",
    )
    .bind(window_minutes)
    .fetch_one(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_ticket(
        status: &str,
        called_at: Option<DateTime<Utc>>,
        returned_at: Option<DateTime<Utc>>,
    ) -> Ticket {
        Ticket {
            id: Uuid::new_v4(),
            seq: 1,
            number: 1,
            group_size: 4,
            status: status.to_owned(),
            created_at: Utc::now(),
            called_at,
            completed_at: None,
            idempotency_key: None,
            returned_at,
            first_viewed_at: None,
            call_request_id: None,
            seated_count: 0,
        }
    }

    #[test]
    fn call_phase_boundaries() {
        let now = DateTime::parse_from_rfc3339("2026-11-12T03:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let waiting = sample_ticket("waiting", None, None);
        assert_eq!(call_phase(&waiting, now), None);
        assert_eq!(waiting.remaining(), 4);

        let completed = sample_ticket("completed", Some(now - TimeDelta::minutes(20)), None);
        assert_eq!(call_phase(&completed, now), None);

        let called_9m59s = sample_ticket("called", Some(now - TimeDelta::seconds(599)), None);
        assert_eq!(call_phase(&called_9m59s, now), Some(CallPhase::Calling));

        let called_10m = sample_ticket("called", Some(now - TimeDelta::seconds(600)), None);
        assert_eq!(call_phase(&called_10m, now), Some(CallPhase::Absent));

        let returned = sample_ticket(
            "called",
            Some(now - TimeDelta::minutes(15)),
            Some(now - TimeDelta::minutes(1)),
        );
        assert_eq!(call_phase(&returned, now), Some(CallPhase::Returned));

        let mut partial_11m = sample_ticket("called", Some(now - TimeDelta::minutes(11)), None);
        partial_11m.seated_count = 2;
        assert_eq!(partial_11m.remaining(), 2);
        assert_eq!(call_phase(&partial_11m, now), Some(CallPhase::Returned));

        let no_called_at = sample_ticket("called", None, None);
        assert_eq!(call_phase(&no_called_at, now), Some(CallPhase::Calling));

        assert_eq!(minutes_since(now - TimeDelta::seconds(599), now), 9);
        assert_eq!(minutes_since(now - TimeDelta::seconds(600), now), 10);
        assert_eq!(minutes_since(now + TimeDelta::seconds(120), now), 0);
        assert_eq!(format_jst_hm(now), "12:00");
    }

    #[test]
    fn front_phase_boundaries() {
        let shown_at = DateTime::parse_from_rfc3339("2026-11-12T03:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            front_phase(shown_at, None, shown_at + TimeDelta::seconds(299)),
            Some(FrontPhase::Showing)
        );
        assert_eq!(
            front_phase(shown_at, None, shown_at + TimeDelta::seconds(300)),
            Some(FrontPhase::Showing)
        );
        assert_eq!(
            front_phase(shown_at, None, shown_at + TimeDelta::seconds(301)),
            None
        );

        let viewed_after = shown_at + TimeDelta::seconds(10);
        assert_eq!(
            front_phase(
                shown_at,
                Some(viewed_after),
                viewed_after + TimeDelta::seconds(7)
            ),
            Some(FrontPhase::Viewed)
        );
        assert_eq!(
            front_phase(
                shown_at,
                Some(viewed_after),
                viewed_after + TimeDelta::seconds(8)
            ),
            Some(FrontPhase::Viewed)
        );
        assert_eq!(
            front_phase(
                shown_at,
                Some(viewed_after),
                viewed_after + TimeDelta::seconds(9)
            ),
            None
        );

        let viewed_before = shown_at - TimeDelta::seconds(5);
        assert_eq!(
            front_phase(
                shown_at,
                Some(viewed_before),
                shown_at + TimeDelta::seconds(20)
            ),
            Some(FrontPhase::Showing)
        );
        assert_eq!(
            front_phase(
                shown_at,
                Some(viewed_before),
                shown_at + TimeDelta::seconds(301)
            ),
            None
        );
    }

    #[test]
    fn wait_estimate_examples() {
        assert_eq!(estimate_wait_minutes(0, 6, 30), Some(5));
        assert_eq!(estimate_wait_minutes(4, 6, 30), Some(25));
        assert_eq!(estimate_wait_minutes(1, 7, 30), Some(10));
        assert_eq!(estimate_wait_minutes(0, 2, 30), None);
    }

    #[test]
    fn display_numbers() {
        let mut active = HashSet::new();
        assert_eq!(next_display_number(0, &active), Some(1));
        assert_eq!(next_display_number(25, &active), Some(26));
        assert_eq!(next_display_number(999, &active), Some(1));
        active.extend([999, 1, 2]);
        assert_eq!(next_display_number(998, &active), Some(3));
        active.extend(1..=999);
        assert_eq!(next_display_number(17, &active), None);
    }

    #[test]
    fn origins() {
        let base = Url::parse("https://example.com").unwrap();
        for value in ["https://example.com", "https://example.com:443/page?q=1"] {
            assert!(is_same_origin(value, &base), "{value}");
        }
        for value in [
            "https://example.com:444",
            "http://example.com",
            "https://example.com.evil.com",
            "null",
            "bad url",
            "https://example.com\nevil",
            " https://example.com",
        ] {
            assert!(!is_same_origin(value, &base), "{value}");
        }
    }

    #[test]
    fn polling_boundaries() {
        for (ahead, interval) in [(0, 15), (2, 15), (3, 30), (10, 30), (11, 60)] {
            assert_eq!(guest_poll_interval("waiting", ahead), interval);
        }
        assert_eq!(guest_poll_interval("called", 99), 15);
        assert_eq!(guest_poll_interval("completed", 0), 0);
    }

    #[test]
    fn group_size_boundaries() {
        for size in [1, MAX_GROUP_SIZE] {
            assert!(validate_group_size(size).is_ok());
        }
        for size in [i32::MIN, -1, 0, 21, i32::MAX] {
            assert!(validate_group_size(size).is_err());
        }
    }
}
