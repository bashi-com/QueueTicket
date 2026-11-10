//! Set TEST_DATABASE_URL to run these tests. Each test uses a temporary schema;
//! existing tables in the database are never truncated or modified.
use std::{collections::HashSet, time::Duration};

use queueticket::{issue_ticket, reset_tickets, transition_ticket, waiting_ahead, AppError};
use sqlx::{postgres::PgPoolOptions, PgPool};
use tokio::task::JoinSet;
use uuid::Uuid;

struct TestDb {
    pool: PgPool,
    admin: PgPool,
    schema: String,
}

impl TestDb {
    async fn new(migrate: bool) -> Option<Self> {
        let url = match std::env::var("TEST_DATABASE_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("Skipping DB test: TEST_DATABASE_URL is not set");
                return None;
            }
        };
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let schema = format!("qt_test_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search_path = schema.clone();
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(Duration::from_secs(5))
            .after_connect(move |connection, _| {
                let search_path = search_path.clone();
                Box::pin(async move {
                    sqlx::query("SELECT set_config('search_path', $1, false)")
                        .bind(search_path)
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        if migrate {
            sqlx::migrate!().run(&pool).await.unwrap();
        }
        Some(Self {
            pool,
            admin,
            schema,
        })
    }

    async fn cleanup(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

#[tokio::test]
async fn fifty_concurrent_tickets_are_unique() {
    let Some(db) = TestDb::new(true).await else {
        return;
    };
    let mut tasks = JoinSet::new();
    for _ in 0..50 {
        let pool = db.pool.clone();
        tasks.spawn(async move { issue_ticket(&pool, 1, Uuid::new_v4()).await.unwrap() });
    }
    let mut numbers = HashSet::new();
    let mut sequences = HashSet::new();
    while let Some(result) = tasks.join_next().await {
        let ticket = result.unwrap();
        assert!(numbers.insert(ticket.number));
        assert!(sequences.insert(ticket.seq));
    }
    assert_eq!(numbers.len(), 50);
    assert_eq!(sequences, (1..=50).collect());
    db.cleanup().await;
}

#[tokio::test]
async fn repeated_request_is_one_ticket_even_concurrently() {
    let Some(db) = TestDb::new(true).await else {
        return;
    };
    let request_id = Uuid::new_v4();
    let (first, second) = tokio::join!(
        issue_ticket(&db.pool, 2, request_id),
        issue_ticket(&db.pool, 2, request_id)
    );
    assert_eq!(first.unwrap().id, second.unwrap().id);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tickets")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let next = issue_ticket(&db.pool, 1, Uuid::new_v4()).await.unwrap();
    assert_eq!((next.number, next.seq), (2, 2));
    db.cleanup().await;
}

#[tokio::test]
async fn stale_status_update_does_not_overwrite() {
    let Some(db) = TestDb::new(true).await else {
        return;
    };
    let ticket = issue_ticket(&db.pool, 1, Uuid::new_v4()).await.unwrap();
    assert_eq!(
        transition_ticket(&db.pool, ticket.id, "waiting", "called")
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        transition_ticket(&db.pool, ticket.id, "waiting", "called")
            .await
            .unwrap(),
        0
    );
    assert!(matches!(
        transition_ticket(&db.pool, ticket.id, "waiting", "completed").await,
        Err(AppError::Validation(_))
    ));
    assert_eq!(
        transition_ticket(&db.pool, ticket.id, "called", "completed")
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        transition_ticket(&db.pool, ticket.id, "called", "completed")
            .await
            .unwrap(),
        0
    );
    let timestamps: (bool, bool) = sqlx::query_as(
        "SELECT called_at IS NOT NULL, completed_at IS NOT NULL FROM tickets WHERE id=$1",
    )
    .bind(ticket.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(timestamps, (true, true));
    db.cleanup().await;
}

#[tokio::test]
async fn wraparound_skips_active_numbers_and_reset_restarts_sequence() {
    let Some(db) = TestDb::new(true).await else {
        return;
    };
    // Keep 1 and 3 active; complete every other issued ticket through 999.
    for number in 1..=999 {
        let ticket = issue_ticket(&db.pool, 1, Uuid::new_v4()).await.unwrap();
        assert_eq!(ticket.number, number);
        if number != 1 && number != 3 {
            transition_ticket(&db.pool, ticket.id, "waiting", "called")
                .await
                .unwrap();
            transition_ticket(&db.pool, ticket.id, "called", "completed")
                .await
                .unwrap();
        }
    }
    let ticket = issue_ticket(&db.pool, 1, Uuid::new_v4()).await.unwrap();
    assert_eq!((ticket.number, ticket.seq), (2, 1000));
    assert_eq!(waiting_ahead(&db.pool, ticket.seq).await.unwrap(), 2);
    let next = issue_ticket(&db.pool, 1, Uuid::new_v4()).await.unwrap();
    assert_eq!((next.number, next.seq), (4, 1001));
    reset_tickets(&db.pool).await.unwrap();
    let first = issue_ticket(&db.pool, 1, Uuid::new_v4()).await.unwrap();
    assert_eq!((first.number, first.seq), (1, 1));
    db.cleanup().await;
}

#[tokio::test]
async fn full_capacity_rejects_new_tickets_but_accepts_retry() {
    let Some(db) = TestDb::new(true).await else {
        return;
    };
    let request_id = Uuid::new_v4();
    let first = issue_ticket(&db.pool, 1, request_id).await.unwrap();
    // Seed the remaining active tickets to exercise the capacity path cheaply.
    sqlx::query("INSERT INTO tickets (number,group_size) SELECT n,1 FROM generate_series(2,999) n")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(matches!(
        issue_ticket(&db.pool, 1, Uuid::new_v4()).await,
        Err(AppError::Capacity)
    ));
    assert_eq!(
        issue_ticket(&db.pool, 1, request_id).await.unwrap().id,
        first.id
    );
    db.cleanup().await;
}

#[tokio::test]
async fn migration_backfills_existing_tickets_in_time_order() {
    let Some(db) = TestDb::new(false).await else {
        return;
    };
    sqlx::raw_sql(include_str!("../migrations/20251127_create_tickets.sql"))
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../migrations/20251216_stronger_timestamp.sql"
    ))
    .execute(&db.pool)
    .await
    .unwrap();
    // Completed display numbers may repeat; latest ticket's number is not MAX(number).
    sqlx::query("INSERT INTO tickets (number,group_size,status,created_at) VALUES (999,1,'completed','2025-01-01'),(1,2,'waiting','2025-01-02')")
        .execute(&db.pool).await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/20261110_ticket_sequence.sql"))
        .execute(&db.pool)
        .await
        .unwrap();
    let rows: Vec<(i64, i32)> = sqlx::query_as("SELECT seq,number FROM tickets ORDER BY seq")
        .fetch_all(&db.pool)
        .await
        .unwrap();
    assert_eq!(rows, vec![(1, 999), (2, 1)]);
    let next = issue_ticket(&db.pool, 1, Uuid::new_v4()).await.unwrap();
    assert_eq!((next.seq, next.number), (3, 2));
    db.cleanup().await;
}
