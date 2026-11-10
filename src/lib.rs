use std::collections::HashSet;

use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use chrono::{DateTime, FixedOffset, Utc};
use sqlx::{FromRow, PgPool};
use url::Url;
use uuid::Uuid;

pub const MAX_GROUP_SIZE: i32 = 20;

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
            "UPDATE tickets SET status=$2, completed_at=NOW() WHERE id=$1 AND status=$3"
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

#[cfg(test)]
mod tests {
    use super::*;

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
