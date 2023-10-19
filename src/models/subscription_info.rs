use crate::models::schema::subscription_info::dsl;
use crate::models::schema::{nwc_pubkeys, subscription_info};
use diesel::prelude::*;
use nostr::{Event, Tag};
use serde::{Deserialize, Serialize};
use web_push::{SubscriptionInfo as WebPushSubscriptionInfo, SubscriptionKeys};

#[derive(Queryable, Insertable, AsChangeset, Serialize, Deserialize, Debug, Clone, PartialEq)]
#[diesel(check_for_backend(diesel::pg::Pg))]
#[diesel(table_name = subscription_info)]
pub struct SubscriptionInfo {
    pub id: String,
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,

    pub created_at: chrono::NaiveDateTime,
}

#[derive(Insertable, AsChangeset)]
#[diesel(table_name = subscription_info)]
pub struct NewSubscriptionInfo<'a> {
    pub id: &'a str,
    pub endpoint: &'a str,
    pub p256dh: &'a str,
    pub auth: &'a str,
}

impl SubscriptionInfo {
    pub fn into_web_push(self) -> WebPushSubscriptionInfo {
        WebPushSubscriptionInfo {
            endpoint: self.endpoint,
            keys: SubscriptionKeys {
                p256dh: self.p256dh,
                auth: self.auth,
            },
        }
    }

    pub fn register(
        conn: &mut PgConnection,
        id: &str,
        endpoint: &str,
        p256dh: &str,
        auth: &str,
    ) -> anyhow::Result<String> {
        let new = NewSubscriptionInfo {
            id,
            endpoint,
            p256dh,
            auth,
        };

        let id: String = diesel::insert_into(subscription_info::table)
            .values(&new)
            .returning(subscription_info::id)
            .on_conflict(subscription_info::id)
            .do_update()
            .set(&new)
            .get_result(conn)?;

        Ok(id)
    }

    pub fn get_all(conn: &mut PgConnection) -> anyhow::Result<Vec<Self>> {
        let items = subscription_info::table.load::<Self>(conn)?;
        Ok(items)
    }

    pub fn find_by_nwc(conn: &mut PgConnection, event: &Event) -> anyhow::Result<Option<Self>> {
        let p_tag = event.tags.iter().find_map(|tag| {
            if let Tag::PubKey(p, _) = tag {
                Some(p.to_owned())
            } else {
                None
            }
        });

        // if no pubkey tag, return None
        let p_tag = match p_tag {
            Some(p_tag) => p_tag,
            None => return Ok(None),
        };

        let result = subscription_info::table
            .inner_join(nwc_pubkeys::table)
            .filter(nwc_pubkeys::author.eq(hex::encode(event.pubkey.serialize())))
            .filter(nwc_pubkeys::tagged.eq(hex::encode(p_tag.serialize())))
            .select(dsl::subscription_info::all_columns())
            .first::<Self>(conn)
            .optional()?;

        Ok(result)
    }
}
