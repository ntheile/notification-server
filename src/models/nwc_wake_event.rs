use crate::models::schema::nwc_wake_events;
use diesel::prelude::*;

#[derive(Insertable)]
#[diesel(table_name = nwc_wake_events)]
struct NewNwcWakeEvent<'a> {
    event_id: &'a str,
    event_created_at: Option<i64>,
}

pub struct NwcWakeEvent;

impl NwcWakeEvent {
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
}
