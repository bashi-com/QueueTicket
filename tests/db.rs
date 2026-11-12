//! Set TEST_DATABASE_URL to run these tests. Each test uses a temporary schema;
//! existing tables in the database are never truncated or modified.
use std::{collections::HashSet, time::Duration};

use chrono::{TimeDelta, Utc};
use queueticket::{
    call_next, call_phase, find_ticket, issue_ticket, mark_first_viewed, mark_returned,
    recent_call_count, reset_tickets, seat_partial, transition_ticket, waiting_ahead, AppError,
    CallPhase, ReturnOutcome, SeatOutcome,
};
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
    sqlx::raw_sql(include_str!("../migrations/20261112_presence.sql"))
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/20261113_partial_seating.sql"))
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

#[tokio::test]
async fn call_next_orders_by_seq_is_idempotent_and_preserves_manual_calls() {
    let Some(db) = TestDb::new(true).await else {
        return;
    };
    assert!(call_next(&db.pool, Uuid::new_v4()).await.unwrap().is_none());

    let t1 = issue_ticket(&db.pool, 1, Uuid::new_v4()).await.unwrap();
    let t2 = issue_ticket(&db.pool, 2, Uuid::new_v4()).await.unwrap();
    let t3 = issue_ticket(&db.pool, 3, Uuid::new_v4()).await.unwrap();

    // Manually call t1 and backdate called_at to verify call_next will not overwrite it.
    transition_ticket(&db.pool, t1.id, "waiting", "called")
        .await
        .unwrap();
    sqlx::query("UPDATE tickets SET called_at = NOW() - INTERVAL '30 minutes' WHERE id=$1")
        .bind(t1.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let fixed_called_at = find_ticket(&db.pool, t1.id).await.unwrap().called_at;

    // Sequential and concurrent idempotency with same request_id calls only t2.
    let req_id = Uuid::new_v4();
    let first = call_next(&db.pool, req_id).await.unwrap().unwrap();
    let second = call_next(&db.pool, req_id).await.unwrap().unwrap();
    assert_eq!(first.id, t2.id);
    assert_eq!(second.id, t2.id);

    let mut same_req_tasks = JoinSet::new();
    for _ in 0..5 {
        let pool = db.pool.clone();
        same_req_tasks.spawn(async move { call_next(&pool, req_id).await.unwrap().unwrap() });
    }
    while let Some(res) = same_req_tasks.join_next().await {
        assert_eq!(res.unwrap().id, t2.id);
    }

    // t3 is still waiting; t1's called_at is unchanged.
    let t1_reloaded = find_ticket(&db.pool, t1.id).await.unwrap();
    assert_eq!(t1_reloaded.called_at, fixed_called_at);
    let t3_reloaded = find_ticket(&db.pool, t3.id).await.unwrap();
    assert_eq!(t3_reloaded.status, "waiting");

    let third = call_next(&db.pool, Uuid::new_v4()).await.unwrap().unwrap();
    assert_eq!(third.id, t3.id);
    assert!(call_next(&db.pool, Uuid::new_v4()).await.unwrap().is_none());

    // 10 waiting tickets called concurrently with 10 distinct request_ids.
    let mut issued_ids = HashSet::new();
    for _ in 0..10 {
        let t = issue_ticket(&db.pool, 1, Uuid::new_v4()).await.unwrap();
        issued_ids.insert(t.id);
    }
    let mut call_tasks = JoinSet::new();
    for _ in 0..10 {
        let pool = db.pool.clone();
        call_tasks
            .spawn(async move { call_next(&pool, Uuid::new_v4()).await.unwrap().unwrap().id });
    }
    let mut called_ids = HashSet::new();
    while let Some(res) = call_tasks.join_next().await {
        assert!(called_ids.insert(res.unwrap()));
    }
    assert_eq!(called_ids, issued_ids);
    db.cleanup().await;
}

#[tokio::test]
async fn mark_returned_and_first_viewed_respect_eligibility_and_idempotency() {
    let Some(db) = TestDb::new(true).await else {
        return;
    };
    let now = Utc::now();
    assert!(matches!(
        mark_returned(&db.pool, Uuid::new_v4(), now, true).await,
        Err(AppError::NotFound)
    ));

    let ticket = issue_ticket(&db.pool, 2, Uuid::new_v4()).await.unwrap();
    assert_eq!(
        mark_returned(&db.pool, ticket.id, now, true).await.unwrap(),
        ReturnOutcome::NotEligible
    );

    transition_ticket(&db.pool, ticket.id, "waiting", "called")
        .await
        .unwrap();
    assert_eq!(
        mark_returned(&db.pool, ticket.id, Utc::now(), true)
            .await
            .unwrap(),
        ReturnOutcome::NotEligible
    );

    sqlx::query("UPDATE tickets SET called_at = NOW() - INTERVAL '11 minutes' WHERE id=$1")
        .bind(ticket.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let return_time = Utc::now();
    assert_eq!(
        mark_returned(&db.pool, ticket.id, return_time, true)
            .await
            .unwrap(),
        ReturnOutcome::Returned
    );
    let saved_return = find_ticket(&db.pool, ticket.id)
        .await
        .unwrap()
        .returned_at
        .unwrap();

    let later = return_time + TimeDelta::minutes(5);
    assert_eq!(
        mark_returned(&db.pool, ticket.id, later, true)
            .await
            .unwrap(),
        ReturnOutcome::AlreadyReturned
    );
    assert_eq!(
        find_ticket(&db.pool, ticket.id).await.unwrap().returned_at,
        Some(saved_return)
    );

    transition_ticket(&db.pool, ticket.id, "called", "completed")
        .await
        .unwrap();
    assert_eq!(
        mark_returned(&db.pool, ticket.id, later, true)
            .await
            .unwrap(),
        ReturnOutcome::NotEligible
    );

    // require_absent = false succeeds immediately after being called
    let t2 = issue_ticket(&db.pool, 1, Uuid::new_v4()).await.unwrap();
    transition_ticket(&db.pool, t2.id, "waiting", "called")
        .await
        .unwrap();
    assert_eq!(
        mark_returned(&db.pool, t2.id, Utc::now(), false)
            .await
            .unwrap(),
        ReturnOutcome::Returned
    );

    // mark_first_viewed keeps the initial timestamp across repeated calls
    mark_first_viewed(&db.pool, t2.id).await.unwrap();
    let first_view = find_ticket(&db.pool, t2.id)
        .await
        .unwrap()
        .first_viewed_at
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    mark_first_viewed(&db.pool, t2.id).await.unwrap();
    let second_view = find_ticket(&db.pool, t2.id)
        .await
        .unwrap()
        .first_viewed_at
        .unwrap();
    assert_eq!(first_view, second_view);

    assert_eq!(recent_call_count(&db.pool, 30).await.unwrap(), 2);
    db.cleanup().await;
}

#[tokio::test]
async fn partial_seating_progresses_toward_completion() {
    let Some(db) = TestDb::new(true).await else {
        return;
    };
    let ticket = issue_ticket(&db.pool, 4, Uuid::new_v4()).await.unwrap();
    transition_ticket(&db.pool, ticket.id, "waiting", "called")
        .await
        .unwrap();

    assert_eq!(
        seat_partial(&db.pool, ticket.id, 2, 0).await.unwrap(),
        SeatOutcome::Partial { remaining: 2 }
    );
    let partial = find_ticket(&db.pool, ticket.id).await.unwrap();
    assert_eq!(partial.status, "called");
    assert_eq!(partial.seated_count, 2);
    assert_eq!(partial.remaining(), 2);
    assert_eq!(call_phase(&partial, Utc::now()), Some(CallPhase::Returned));

    // Retrying with the stale expected_seated value must not double-count.
    assert_eq!(
        seat_partial(&db.pool, ticket.id, 2, 0).await.unwrap(),
        SeatOutcome::Conflict
    );
    assert_eq!(
        find_ticket(&db.pool, ticket.id).await.unwrap().seated_count,
        2
    );

    assert_eq!(
        seat_partial(&db.pool, ticket.id, 2, 2).await.unwrap(),
        SeatOutcome::Completed
    );
    let completed = find_ticket(&db.pool, ticket.id).await.unwrap();
    assert_eq!(completed.status, "completed");
    assert!(completed.completed_at.is_some());
    assert_eq!(completed.seated_count, 4);
    db.cleanup().await;
}

#[tokio::test]
async fn partial_seating_conflicts_validation_and_missing_tickets() {
    let Some(db) = TestDb::new(true).await else {
        return;
    };
    let ticket = issue_ticket(&db.pool, 4, Uuid::new_v4()).await.unwrap();
    transition_ticket(&db.pool, ticket.id, "waiting", "called")
        .await
        .unwrap();
    seat_partial(&db.pool, ticket.id, 2, 0).await.unwrap();

    // Exceeding the remaining seats is rejected without changing anything.
    assert_eq!(
        seat_partial(&db.pool, ticket.id, 3, 2).await.unwrap(),
        SeatOutcome::Conflict
    );
    assert_eq!(
        find_ticket(&db.pool, ticket.id).await.unwrap().seated_count,
        2
    );

    // A waiting ticket cannot be seated.
    let waiting = issue_ticket(&db.pool, 2, Uuid::new_v4()).await.unwrap();
    assert_eq!(
        seat_partial(&db.pool, waiting.id, 1, 0).await.unwrap(),
        SeatOutcome::Conflict
    );

    // Unknown id -> NotFound.
    assert!(matches!(
        seat_partial(&db.pool, Uuid::new_v4(), 1, 0).await,
        Err(AppError::NotFound)
    ));

    // Non-positive seat count -> Validation.
    assert!(matches!(
        seat_partial(&db.pool, ticket.id, 0, 2).await,
        Err(AppError::Validation(_))
    ));

    // A ticket that is already absent (11 minutes) still accepts partial seating
    // and its phase becomes Returned.
    let absent = issue_ticket(&db.pool, 4, Uuid::new_v4()).await.unwrap();
    transition_ticket(&db.pool, absent.id, "waiting", "called")
        .await
        .unwrap();
    sqlx::query("UPDATE tickets SET called_at = NOW() - INTERVAL '11 minutes' WHERE id=$1")
        .bind(absent.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let before = find_ticket(&db.pool, absent.id).await.unwrap();
    assert_eq!(call_phase(&before, Utc::now()), Some(CallPhase::Absent));
    assert_eq!(
        seat_partial(&db.pool, absent.id, 1, 0).await.unwrap(),
        SeatOutcome::Partial { remaining: 3 }
    );
    let after = find_ticket(&db.pool, absent.id).await.unwrap();
    assert_eq!(call_phase(&after, Utc::now()), Some(CallPhase::Returned));
    db.cleanup().await;
}

#[tokio::test]
async fn concurrent_partial_seating_counts_only_once() {
    let Some(db) = TestDb::new(true).await else {
        return;
    };
    let ticket = issue_ticket(&db.pool, 4, Uuid::new_v4()).await.unwrap();
    transition_ticket(&db.pool, ticket.id, "waiting", "called")
        .await
        .unwrap();

    let mut tasks = JoinSet::new();
    for _ in 0..5 {
        let pool = db.pool.clone();
        let id = ticket.id;
        tasks.spawn(async move { seat_partial(&pool, id, 1, 0).await.unwrap() });
    }
    let mut successes = 0;
    while let Some(result) = tasks.join_next().await {
        if result.unwrap() == (SeatOutcome::Partial { remaining: 3 }) {
            successes += 1;
        }
    }
    assert_eq!(successes, 1);
    assert_eq!(
        find_ticket(&db.pool, ticket.id).await.unwrap().seated_count,
        1
    );
    db.cleanup().await;
}

#[tokio::test]
async fn completing_a_called_ticket_records_full_seated_count() {
    let Some(db) = TestDb::new(true).await else {
        return;
    };
    let ticket = issue_ticket(&db.pool, 4, Uuid::new_v4()).await.unwrap();
    transition_ticket(&db.pool, ticket.id, "waiting", "called")
        .await
        .unwrap();
    assert_eq!(
        transition_ticket(&db.pool, ticket.id, "called", "completed")
            .await
            .unwrap(),
        1
    );
    let completed = find_ticket(&db.pool, ticket.id).await.unwrap();
    assert_eq!(completed.seated_count, completed.group_size);
    assert_eq!(completed.remaining(), 0);
    db.cleanup().await;
}
