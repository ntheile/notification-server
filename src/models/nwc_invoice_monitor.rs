use crate::models::schema::nwc_invoice_monitors;
use chrono::{DateTime, Duration, Utc};
use diesel::prelude::*;

#[allow(dead_code)]
#[derive(Queryable, Debug, Clone)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct NwcInvoiceMonitor {
    pub id: String,
    pub request_event_id: String,
    pub client_pubkey: String,
    pub wallet_service_pubkey: String,
    pub relay: String,
    pub expires_at: DateTime<Utc>,
    pub next_wake_at: DateTime<Utc>,
    pub wake_count: i32,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Insertable)]
#[diesel(table_name = nwc_invoice_monitors)]
struct NewNwcInvoiceMonitor<'a> {
    id: &'a str,
    request_event_id: &'a str,
    client_pubkey: &'a str,
    wallet_service_pubkey: &'a str,
    relay: &'a str,
    expires_at: DateTime<Utc>,
    next_wake_at: DateTime<Utc>,
}

impl NwcInvoiceMonitor {
    #[allow(clippy::too_many_arguments)]
    pub fn enable(
        conn: &mut PgConnection,
        id: &str,
        request_event_id: &str,
        client_pubkey: &str,
        wallet_service_pubkey: &str,
        relay: &str,
        expires_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let new = NewNwcInvoiceMonitor {
            id,
            request_event_id,
            client_pubkey,
            wallet_service_pubkey,
            relay,
            expires_at,
            next_wake_at: Utc::now() + Duration::seconds(5),
        };
        diesel::insert_into(nwc_invoice_monitors::table)
            .values(&new)
            .on_conflict((
                nwc_invoice_monitors::id,
                nwc_invoice_monitors::request_event_id,
                nwc_invoice_monitors::relay,
            ))
            .do_update()
            .set((
                nwc_invoice_monitors::expires_at.eq(expires_at),
                nwc_invoice_monitors::enabled.eq(true),
                nwc_invoice_monitors::updated_at.eq(diesel::dsl::now),
            ))
            .execute(conn)?;
        Ok(())
    }

    pub fn disable(
        conn: &mut PgConnection,
        id: &str,
        request_event_id: &str,
        wallet_service_pubkey: &str,
        relay: &str,
    ) -> anyhow::Result<usize> {
        Ok(diesel::update(
            nwc_invoice_monitors::table
                .filter(nwc_invoice_monitors::id.eq(id))
                .filter(nwc_invoice_monitors::request_event_id.eq(request_event_id))
                .filter(nwc_invoice_monitors::wallet_service_pubkey.eq(wallet_service_pubkey))
                .filter(nwc_invoice_monitors::relay.eq(relay)),
        )
        .set((
            nwc_invoice_monitors::enabled.eq(false),
            nwc_invoice_monitors::updated_at.eq(diesel::dsl::now),
        ))
        .execute(conn)?)
    }

    pub fn claim_due(conn: &mut PgConnection, maximum: i64) -> anyhow::Result<Vec<Self>> {
        let now = Utc::now();
        conn.transaction(|conn| {
            let due = nwc_invoice_monitors::table
                .filter(nwc_invoice_monitors::enabled.eq(true))
                .filter(nwc_invoice_monitors::expires_at.gt(now))
                .filter(nwc_invoice_monitors::wake_count.lt(24))
                .filter(nwc_invoice_monitors::next_wake_at.le(now))
                .order(nwc_invoice_monitors::next_wake_at.asc())
                .limit(maximum)
                .for_update()
                .skip_locked()
                .load::<Self>(conn)?;
            for monitor in &due {
                let next_count = monitor.wake_count.saturating_add(1).min(64);
                let delay_seconds = match next_count {
                    0..=2 => 5,
                    3 => 10,
                    4 => 20,
                    5 => 30,
                    6 => 60,
                    7 => 120,
                    _ => 300,
                };
                diesel::update(
                    nwc_invoice_monitors::table
                        .filter(nwc_invoice_monitors::id.eq(&monitor.id))
                        .filter(
                            nwc_invoice_monitors::request_event_id.eq(&monitor.request_event_id),
                        )
                        .filter(nwc_invoice_monitors::relay.eq(&monitor.relay)),
                )
                .set((
                    nwc_invoice_monitors::wake_count.eq(next_count),
                    nwc_invoice_monitors::next_wake_at.eq(now + Duration::seconds(delay_seconds)),
                    nwc_invoice_monitors::updated_at.eq(diesel::dsl::now),
                ))
                .execute(conn)?;
            }
            Ok(due)
        })
    }

    pub fn disable_expired(conn: &mut PgConnection) -> anyhow::Result<usize> {
        Ok(diesel::update(
            nwc_invoice_monitors::table
                .filter(nwc_invoice_monitors::enabled.eq(true))
                .filter(nwc_invoice_monitors::expires_at.le(Utc::now())),
        )
        .set((
            nwc_invoice_monitors::enabled.eq(false),
            nwc_invoice_monitors::updated_at.eq(diesel::dsl::now),
        ))
        .execute(conn)?)
    }

    pub fn disable_exhausted(conn: &mut PgConnection) -> anyhow::Result<usize> {
        Ok(diesel::update(
            nwc_invoice_monitors::table
                .filter(nwc_invoice_monitors::enabled.eq(true))
                .filter(nwc_invoice_monitors::wake_count.ge(24)),
        )
        .set((
            nwc_invoice_monitors::enabled.eq(false),
            nwc_invoice_monitors::updated_at.eq(diesel::dsl::now),
        ))
        .execute(conn)?)
    }
}
