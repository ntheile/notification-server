use crate::models::schema::nwc_wake_events;
use diesel::prelude::*;
use diesel::sql_types::BigInt;

#[derive(Insertable)]
#[diesel(table_name = nwc_wake_events)]
struct NewNwcWakeEvent<'a> {
    event_id: &'a str,
    event_created_at: Option<i64>,
}

pub struct NwcWakeEvent;

impl NwcWakeEvent {
    pub fn exists(conn: &mut PgConnection, event_id: &str) -> anyhow::Result<bool> {
        let exists = diesel::select(diesel::dsl::exists(
            nwc_wake_events::table.filter(nwc_wake_events::event_id.eq(event_id)),
        ))
        .get_result(conn)?;

        Ok(exists)
    }

    pub fn record_once(
        conn: &mut PgConnection,
        event_id: &str,
        event_created_at: Option<u64>,
    ) -> anyhow::Result<bool> {
        let event_created_at = event_created_at.map(i64::try_from).transpose()?;
        let new = NewNwcWakeEvent {
            event_id,
            event_created_at,
        };

        let inserted = diesel::insert_into(nwc_wake_events::table)
            .values(&new)
            .on_conflict(nwc_wake_events::event_id)
            .do_nothing()
            .execute(conn)?;

        Ok(inserted > 0)
    }

    pub fn prune_older_than(conn: &mut PgConnection, retention_secs: u64) -> anyhow::Result<usize> {
        let retention_secs = i64::try_from(retention_secs)?;
        let deleted = diesel::sql_query(
            "DELETE FROM nwc_wake_events WHERE received_at < NOW() - ($1 * INTERVAL '1 second')",
        )
        .bind::<BigInt, _>(retention_secs)
        .execute(conn)?;

        Ok(deleted)
    }
}
