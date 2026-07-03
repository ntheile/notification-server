use crate::models::nwc_pubkey::NwcFilterInfo;
use crate::models::schema::apns_nwc_registrations;
use diesel::prelude::*;
use nostr::key::XOnlyPublicKey;
use nostr::{Event, Tag};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Queryable, Insertable, AsChangeset, Serialize, Deserialize, Debug, Clone, PartialEq)]
#[diesel(check_for_backend(diesel::pg::Pg))]
#[diesel(table_name = apns_nwc_registrations)]
pub struct ApnsNwcRegistration {
    pub id: String,
    pub device_token: String,
    pub bundle_id: String,
    pub environment: String,
    pub author: String,
    pub tagged: String,
    pub relay: String,
    pub name: String,
    pub enabled: bool,
    pub created_at: chrono::NaiveDateTime,
    pub updated_at: chrono::NaiveDateTime,
}

#[derive(Insertable, AsChangeset)]
#[diesel(table_name = apns_nwc_registrations)]
pub struct NewApnsNwcRegistration<'a> {
    pub id: &'a str,
    pub device_token: &'a str,
    pub bundle_id: &'a str,
    pub environment: &'a str,
    pub author: &'a str,
    pub tagged: &'a str,
    pub relay: &'a str,
    pub name: &'a str,
    pub enabled: bool,
}

impl ApnsNwcRegistration {
    #[allow(clippy::too_many_arguments)]
    pub fn register(
        conn: &mut PgConnection,
        id: &str,
        device_token: &str,
        bundle_id: &str,
        environment: &str,
        author: &str,
        tagged: &str,
        relay: &str,
        name: &str,
        enabled: bool,
    ) -> anyhow::Result<()> {
        let new = NewApnsNwcRegistration {
            id,
            device_token,
            bundle_id,
            environment,
            author,
            tagged,
            relay,
            name,
            enabled,
        };

        diesel::insert_into(apns_nwc_registrations::table)
            .values(&new)
            .on_conflict((
                apns_nwc_registrations::id,
                apns_nwc_registrations::author,
                apns_nwc_registrations::tagged,
                apns_nwc_registrations::relay,
            ))
            .do_update()
            .set((
                apns_nwc_registrations::device_token.eq(device_token),
                apns_nwc_registrations::bundle_id.eq(bundle_id),
                apns_nwc_registrations::environment.eq(environment),
                apns_nwc_registrations::name.eq(name),
                apns_nwc_registrations::enabled.eq(enabled),
                apns_nwc_registrations::updated_at.eq(diesel::dsl::now),
            ))
            .execute(conn)?;

        Ok(())
    }

    pub fn get_filter_info(conn: &mut PgConnection) -> anyhow::Result<NwcFilterInfo> {
        conn.transaction(|conn| {
            let authors = apns_nwc_registrations::table
                .filter(apns_nwc_registrations::enabled.eq(true))
                .select(apns_nwc_registrations::author)
                .distinct()
                .load::<String>(conn)?;

            let tagged = apns_nwc_registrations::table
                .filter(apns_nwc_registrations::enabled.eq(true))
                .select(apns_nwc_registrations::tagged)
                .distinct()
                .load::<String>(conn)?
                .into_iter()
                .flat_map(|s| XOnlyPublicKey::from_str(&s).ok())
                .collect::<Vec<_>>();

            let relays = apns_nwc_registrations::table
                .filter(apns_nwc_registrations::enabled.eq(true))
                .select(apns_nwc_registrations::relay)
                .distinct()
                .load::<String>(conn)?;

            Ok(NwcFilterInfo {
                authors,
                tagged,
                relays,
            })
        })
    }

    pub fn find_by_nwc_event(
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

        Self::find_by_nwc(conn, event.pubkey, p_tag, relay)
    }

    pub fn find_by_nwc(
        conn: &mut PgConnection,
        author: XOnlyPublicKey,
        tagged: XOnlyPublicKey,
        relay: &str,
    ) -> anyhow::Result<Vec<Self>> {
        let relay_without_trailing_slash = relay.trim_end_matches('/');
        let relay_with_trailing_slash = format!("{relay_without_trailing_slash}/");

        let items = apns_nwc_registrations::table
            .filter(apns_nwc_registrations::enabled.eq(true))
            .filter(apns_nwc_registrations::author.eq(hex::encode(author.serialize())))
            .filter(apns_nwc_registrations::tagged.eq(hex::encode(tagged.serialize())))
            .filter(
                apns_nwc_registrations::relay
                    .eq(relay_without_trailing_slash)
                    .or(apns_nwc_registrations::relay.eq(relay_with_trailing_slash)),
            )
            .load::<Self>(conn)?;

        Ok(items)
    }
}
