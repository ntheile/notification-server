use crate::models::nwc_pubkey::NwcFilterInfo;
use crate::models::schema::nwc_push_registrations;
use diesel::prelude::*;
use nostr::key::XOnlyPublicKey;
use nostr::{Event, Tag};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

pub const PUSH_SERVICE_APNS: &str = "apns";

#[derive(Queryable, Insertable, AsChangeset, Serialize, Deserialize, Debug, Clone, PartialEq)]
#[diesel(check_for_backend(diesel::pg::Pg))]
#[diesel(table_name = nwc_push_registrations)]
pub struct NwcPushRegistration {
    pub id: String,
    pub push_service: String,
    pub push_token: String,
    pub app_id: String,
    pub environment: String,
    pub author: String,
    pub tagged: String,
    pub relay: String,
    pub name: String,
    pub enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Insertable, AsChangeset)]
#[diesel(table_name = nwc_push_registrations)]
pub struct NewNwcPushRegistration<'a> {
    pub id: &'a str,
    pub push_service: &'a str,
    pub push_token: &'a str,
    pub app_id: &'a str,
    pub environment: &'a str,
    pub author: &'a str,
    pub tagged: &'a str,
    pub relay: &'a str,
    pub name: &'a str,
    pub enabled: bool,
}

impl NwcPushRegistration {
    #[allow(clippy::too_many_arguments)]
    pub fn register(
        conn: &mut PgConnection,
        id: &str,
        push_service: &str,
        push_token: &str,
        app_id: &str,
        environment: &str,
        author: &str,
        tagged: &str,
        relay: &str,
        name: &str,
        enabled: bool,
    ) -> anyhow::Result<()> {
        let new = NewNwcPushRegistration {
            id,
            push_service,
            push_token,
            app_id,
            environment,
            author,
            tagged,
            relay,
            name,
            enabled,
        };

        diesel::insert_into(nwc_push_registrations::table)
            .values(&new)
            .on_conflict((
                nwc_push_registrations::id,
                nwc_push_registrations::push_service,
                nwc_push_registrations::author,
                nwc_push_registrations::tagged,
                nwc_push_registrations::relay,
            ))
            .do_update()
            .set((
                nwc_push_registrations::push_token.eq(push_token),
                nwc_push_registrations::app_id.eq(app_id),
                nwc_push_registrations::environment.eq(environment),
                nwc_push_registrations::name.eq(name),
                nwc_push_registrations::enabled.eq(enabled),
                nwc_push_registrations::updated_at.eq(diesel::dsl::now),
            ))
            .execute(conn)?;

        Ok(())
    }

    pub fn get_filter_info(conn: &mut PgConnection) -> anyhow::Result<NwcFilterInfo> {
        conn.transaction(|conn| {
            let authors = nwc_push_registrations::table
                .filter(nwc_push_registrations::enabled.eq(true))
                .select(nwc_push_registrations::author)
                .distinct()
                .load::<String>(conn)?;

            let tagged = nwc_push_registrations::table
                .filter(nwc_push_registrations::enabled.eq(true))
                .select(nwc_push_registrations::tagged)
                .distinct()
                .load::<String>(conn)?
                .into_iter()
                .flat_map(|s| XOnlyPublicKey::from_str(&s).ok())
                .collect::<Vec<_>>();

            let relays = nwc_push_registrations::table
                .filter(nwc_push_registrations::enabled.eq(true))
                .select(nwc_push_registrations::relay)
                .distinct()
                .load::<String>(conn)?;

            Ok(NwcFilterInfo {
                authors,
                tagged,
                relays,
            })
        })
    }

    pub fn disable(conn: &mut PgConnection, registration: &Self) -> anyhow::Result<()> {
        diesel::update(
            nwc_push_registrations::table
                .filter(nwc_push_registrations::id.eq(&registration.id))
                .filter(nwc_push_registrations::push_service.eq(&registration.push_service))
                .filter(nwc_push_registrations::author.eq(&registration.author))
                .filter(nwc_push_registrations::tagged.eq(&registration.tagged))
                .filter(nwc_push_registrations::relay.eq(&registration.relay)),
        )
        .set((
            nwc_push_registrations::enabled.eq(false),
            nwc_push_registrations::updated_at.eq(diesel::dsl::now),
        ))
        .execute(conn)?;

        Ok(())
    }

    pub fn unregister(
        conn: &mut PgConnection,
        id: &str,
        push_service: &str,
        author: &str,
        tagged: &str,
        relay: &str,
    ) -> anyhow::Result<usize> {
        let deleted = diesel::delete(
            nwc_push_registrations::table
                .filter(nwc_push_registrations::id.eq(id))
                .filter(nwc_push_registrations::push_service.eq(push_service))
                .filter(nwc_push_registrations::author.eq(author))
                .filter(nwc_push_registrations::tagged.eq(tagged))
                .filter(nwc_push_registrations::relay.eq(relay)),
        )
        .execute(conn)?;

        Ok(deleted)
    }

    pub fn find_apns_by_nwc_event(
        conn: &mut PgConnection,
        event: &Event,
        relay: &str,
    ) -> anyhow::Result<Vec<Self>> {
        let p_tag = event.tags.iter().find_map(|tag| {
            if let Tag::PubKey(p, _) = tag {
                Some(p.to_owned())
            } else {
                None
            }
        });

        let Some(p_tag) = p_tag else {
            return Ok(Vec::new());
        };

        Self::find_apns_by_nwc(conn, event.pubkey, p_tag, relay)
    }

    pub fn find_apns_by_nwc(
        conn: &mut PgConnection,
        author: XOnlyPublicKey,
        tagged: XOnlyPublicKey,
        relay: &str,
    ) -> anyhow::Result<Vec<Self>> {
        Self::find_by_nwc_service(conn, PUSH_SERVICE_APNS, author, tagged, relay)
    }

    fn find_by_nwc_service(
        conn: &mut PgConnection,
        push_service: &str,
        author: XOnlyPublicKey,
        tagged: XOnlyPublicKey,
        relay: &str,
    ) -> anyhow::Result<Vec<Self>> {
        let relay_without_trailing_slash = relay.trim_end_matches('/');
        let relay_with_trailing_slash = format!("{relay_without_trailing_slash}/");

        let items = nwc_push_registrations::table
            .filter(nwc_push_registrations::enabled.eq(true))
            .filter(nwc_push_registrations::push_service.eq(push_service))
            .filter(nwc_push_registrations::author.eq(hex::encode(author.serialize())))
            .filter(nwc_push_registrations::tagged.eq(hex::encode(tagged.serialize())))
            .filter(
                nwc_push_registrations::relay
                    .eq(relay_without_trailing_slash)
                    .or(nwc_push_registrations::relay.eq(relay_with_trailing_slash)),
            )
            .load::<Self>(conn)?;

        Ok(items)
    }
}
