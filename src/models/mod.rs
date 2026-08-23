use diesel_migrations::{embed_migrations, EmbeddedMigrations};

pub mod nwc_invoice_monitor;
pub mod nwc_pubkey;
pub mod nwc_push_registration;
pub mod nwc_wake_event;
mod schema;
pub mod subscription_info;

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

#[cfg(test)]
mod test {
    use super::*;
    use crate::models::nwc_invoice_monitor::NwcInvoiceMonitor;
    use crate::models::nwc_pubkey::NwcPubkeys;
    use crate::models::nwc_push_registration::{NwcPushRegistration, PUSH_SERVICE_APNS};
    use crate::models::subscription_info::SubscriptionInfo;
    use diesel::prelude::*;
    use diesel::r2d2::{ConnectionManager, Pool};
    use diesel_migrations::MigrationHarness;
    use std::sync::Mutex;

    const ENDPOINT: &str = "https://updates.push.services.mozilla.com/wpush/v1/TOKEN";
    const AUTH: &str = "####secret####";
    const P256DH: &str = "####public_key####";

    const AUTHOR: &str = "11411cfd44e2127c841cac432d7ac582179316ccc1f3d59e0ebec33117e2c70c";
    const TAGGED: &str = "b5069f40b59c7e3bf924e381692c9758620f1290017047de78567c03b98eddd3";
    const RELAY: &str = "wss://nostr.mutinywallet.com";
    static TEST_DB_LOCK: Mutex<()> = Mutex::new(());

    fn lock_test_database() -> std::sync::MutexGuard<'static, ()> {
        TEST_DB_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn init_db_pool() -> Pool<ConnectionManager<PgConnection>> {
        dotenv::dotenv().ok();
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let manager = ConnectionManager::<PgConnection>::new(url);
        let db_pool = Pool::builder()
            .build(manager)
            .expect("Could not build connection pool");

        // run migrations
        let mut connection = db_pool.get().unwrap();
        connection
            .run_pending_migrations(MIGRATIONS)
            .expect("migrations could not run");

        db_pool
    }

    fn clear_database(db_pool: &Pool<ConnectionManager<PgConnection>>) {
        let conn = &mut db_pool.get().unwrap();

        conn.transaction::<_, anyhow::Error, _>(|conn| {
            diesel::delete(schema::nwc_invoice_monitors::table).execute(conn)?;
            diesel::delete(schema::nwc_wake_events::table).execute(conn)?;
            diesel::delete(schema::nwc_push_registrations::table).execute(conn)?;
            diesel::delete(schema::nwc_pubkeys::table).execute(conn)?;
            diesel::delete(schema::subscription_info::table).execute(conn)?;
            Ok(())
        })
        .unwrap();
    }

    #[tokio::test]
    async fn test_register() {
        let _guard = lock_test_database();
        let db_pool = init_db_pool();
        clear_database(&db_pool);

        let mut conn = db_pool.get().unwrap();
        let expected_id = "dummy";
        let id =
            SubscriptionInfo::register(&mut conn, expected_id, ENDPOINT, P256DH, AUTH).unwrap();

        assert_eq!(id, expected_id);

        clear_database(&db_pool);
    }

    #[tokio::test]
    async fn test_register_update() {
        let _guard = lock_test_database();
        let db_pool = init_db_pool();
        clear_database(&db_pool);

        let mut conn = db_pool.get().unwrap();
        let expected_id = "dummy";
        let id =
            SubscriptionInfo::register(&mut conn, expected_id, ENDPOINT, P256DH, AUTH).unwrap();

        assert_eq!(id, expected_id);

        // test update
        let id = SubscriptionInfo::register(
            &mut conn,
            expected_id,
            "https://updates.push.services.mozilla.com/wpush/v1/NEW_TOKEN",
            "####new_secret####",
            "####new_public_key####",
        )
        .unwrap();

        assert_eq!(id, expected_id);

        clear_database(&db_pool);
    }

    #[tokio::test]
    async fn test_register_nwc() {
        let _guard = lock_test_database();
        let db_pool = init_db_pool();
        clear_database(&db_pool);

        let mut conn = db_pool.get().unwrap();
        let expected_id = "dummy";
        let id =
            SubscriptionInfo::register(&mut conn, expected_id, ENDPOINT, P256DH, AUTH).unwrap();

        assert_eq!(id, expected_id);

        let name = "test";
        NwcPubkeys::register(&mut conn, expected_id, AUTHOR, TAGGED, RELAY, name).unwrap();

        let Some((sub_info, found_name)) = SubscriptionInfo::find_by_nwc(
            &mut conn,
            AUTHOR.parse().unwrap(),
            TAGGED.parse().unwrap(),
        )
        .unwrap() else {
            panic!("subscription info not found")
        };

        assert_eq!(found_name, name);
        assert_eq!(sub_info.id, expected_id);

        clear_database(&db_pool);
    }

    #[tokio::test]
    async fn test_unregister_nwc_push_is_idempotent() {
        let _guard = lock_test_database();
        let db_pool = init_db_pool();
        clear_database(&db_pool);

        let mut conn = db_pool.get().unwrap();
        NwcPushRegistration::register(
            &mut conn,
            "install-id",
            PUSH_SERVICE_APNS,
            "device-token",
            "com.example.wallet",
            "sandbox",
            AUTHOR,
            TAGGED,
            RELAY,
            "test",
            true,
        )
        .unwrap();

        let registrations = NwcPushRegistration::find_apns_by_nwc(
            &mut conn,
            AUTHOR.parse().unwrap(),
            TAGGED.parse().unwrap(),
            RELAY,
        )
        .unwrap();
        assert_eq!(registrations.len(), 1);

        let deleted = NwcPushRegistration::unregister(
            &mut conn,
            "install-id",
            PUSH_SERVICE_APNS,
            AUTHOR,
            TAGGED,
            RELAY,
        )
        .unwrap();
        assert_eq!(deleted, 1);

        let deleted_again = NwcPushRegistration::unregister(
            &mut conn,
            "install-id",
            PUSH_SERVICE_APNS,
            AUTHOR,
            TAGGED,
            RELAY,
        )
        .unwrap();
        assert_eq!(deleted_again, 0);

        clear_database(&db_pool);
    }

    #[tokio::test]
    async fn invoice_monitor_is_idempotent_and_wallet_bound() {
        let _guard = lock_test_database();
        let db_pool = init_db_pool();
        clear_database(&db_pool);
        let mut conn = db_pool.get().unwrap();
        let event_id = "ab".repeat(32);
        let expires_at = chrono::Utc::now() + chrono::Duration::hours(1);

        NwcInvoiceMonitor::enable(
            &mut conn,
            "install-id",
            &event_id,
            AUTHOR,
            TAGGED,
            RELAY,
            expires_at,
        )
        .unwrap();
        NwcInvoiceMonitor::enable(
            &mut conn,
            "install-id",
            &event_id,
            AUTHOR,
            TAGGED,
            RELAY,
            expires_at,
        )
        .unwrap();

        assert_eq!(
            schema::nwc_invoice_monitors::table
                .count()
                .get_result::<i64>(&mut conn)
                .unwrap(),
            1
        );
        assert_eq!(
            NwcInvoiceMonitor::disable(&mut conn, "install-id", &event_id, AUTHOR, RELAY,).unwrap(),
            0
        );
        assert_eq!(
            NwcInvoiceMonitor::disable(&mut conn, "install-id", &event_id, TAGGED, RELAY,).unwrap(),
            1
        );
        clear_database(&db_pool);
    }
}
