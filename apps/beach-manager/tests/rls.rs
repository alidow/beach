//! Focused RLS policy checks using a limited DB role and FORCE RLS.
//!
//! Run with a real Postgres:
//!   DATABASE_URL=postgres://postgres:postgres@localhost:5432/beach_manager \
//!   cargo test -p beach-manager -- --ignored rls_policies_enforced

use sqlx::{postgres::{PgConnectOptions, PgPoolOptions}, Connection, Executor};
use uuid::Uuid;

#[ignore]
#[tokio::test]
async fn rls_policies_enforced() {
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL is required");

    // Admin pool to run migrations and set up role/privileges.
    let admin_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("connect admin");

    sqlx::migrate!("./migrations")
        .run(&admin_pool)
        .await
        .expect("migrations");

    // Ensure a limited role exists and has privileges but no BYPASSRLS.
    sqlx::query(
        r#"
        DO $$
        BEGIN
            IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'pb_rls_tester') THEN
                CREATE ROLE pb_rls_tester WITH LOGIN PASSWORD 'pbtest' NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
            END IF;
        END$$;
        "#,
    )
    .execute(&admin_pool)
    .await
    .expect("create role");

    // Grants for schema/tables/enums used in this test.
    for sql in [
        "GRANT USAGE ON SCHEMA public TO pb_rls_tester",
        "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO pb_rls_tester",
        "GRANT USAGE ON TYPE session_kind TO pb_rls_tester",
        "GRANT USAGE ON TYPE harness_type TO pb_rls_tester",
        "GRANT USAGE ON TYPE controller_event_type TO pb_rls_tester",
        "ALTER TABLE session FORCE ROW LEVEL SECURITY",
    ] {
        sqlx::query(sql).execute(&admin_pool).await.expect(sql);
    }

    // Build limited role connection options from DATABASE_URL.
    let opts: PgConnectOptions = db_url.parse().expect("parse DATABASE_URL");
    let limited_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(opts.username("pb_rls_tester").password("pbtest"))
        .await
        .expect("connect limited role");

    // Prepare IDs for two beaches.
    let beach_a = Uuid::new_v4();
    let beach_b = Uuid::new_v4();
    let sess = Uuid::new_v4();
    let harness = Uuid::new_v4();

    // With wrong/absent GUC, insert should fail the WITH CHECK policy.
    let insert_stmt = r#"
        INSERT INTO session (
            id, private_beach_id, origin_session_id, harness_id, kind,
            location_hint, capabilities, metadata, harness_type, last_seen_at
        ) VALUES ($1, $2, $3, $4, 'terminal', 'us-test-1', '[]'::jsonb, '{}'::jsonb, 'terminal_shim', NOW())
    "#;

    // Explicitly set a mismatched GUC (beach A) while inserting row for beach B.
    let mut conn = limited_pool.acquire().await.expect("acquire");
    sqlx::query("SELECT set_config('beach.private_beach_id', $1, true)")
        .bind(beach_a.to_string())
        .execute(&mut *conn)
        .await
        .expect("set guc A");

    let err = sqlx::query(insert_stmt)
        .bind(Uuid::new_v4())
        .bind(beach_b)
        .bind(sess)
        .bind(harness)
        .execute(&mut *conn)
        .await
        .expect_err("insert should be blocked by RLS");
    // SQLSTATE 42501 (insufficient_privilege) is typical for RLS check failures.
    assert!(format!("{err}").contains("row-level security") || format!("{err}").contains("42501"));

    // Now set GUC to beach B and insert should succeed.
    sqlx::query("SELECT set_config('beach.private_beach_id', $1, true)")
        .bind(beach_b.to_string())
        .execute(&mut *conn)
        .await
        .expect("set guc B");

    sqlx::query(insert_stmt)
        .bind(Uuid::new_v4())
        .bind(beach_b)
        .bind(sess)
        .bind(harness)
        .execute(&mut *conn)
        .await
        .expect("insert with correct GUC");

    // SELECT with mismatched GUC returns 0 rows.
    sqlx::query("SELECT set_config('beach.private_beach_id', $1, true)")
        .bind(beach_a.to_string())
        .execute(&mut *conn)
        .await
        .expect("set guc A again");

    let rows: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM session WHERE private_beach_id = $1")
        .bind(beach_b)
        .fetch_all(&mut *conn)
        .await
        .expect("select under RLS");
    assert!(rows.is_empty(), "RLS should hide rows for other beaches");
}

