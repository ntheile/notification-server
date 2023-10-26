use crate::models::schema::nwc_pubkeys;
use diesel::prelude::*;
use nostr::key::XOnlyPublicKey;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Queryable, Insertable, AsChangeset, Serialize, Deserialize, Debug, Clone, PartialEq)]
#[diesel(check_for_backend(diesel::pg::Pg))]
#[diesel(table_name = nwc_pubkeys)]
pub struct NwcPubkeys {
    pub id: String,
    pub author: String,
    pub tagged: String,
    pub relay: String,
    pub name: String,

    pub created_at: chrono::NaiveDateTime,
}

#[derive(Insertable, AsChangeset)]
#[diesel(table_name = nwc_pubkeys)]
pub struct NewNwcPubkeys<'a> {
    pub id: &'a str,
    pub author: &'a str,
    pub tagged: &'a str,
    pub relay: &'a str,
    pub name: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NwcFilterInfo {
    pub authors: Vec<String>,
    pub tagged: Vec<XOnlyPublicKey>,
    pub relays: Vec<String>,
}

impl NwcPubkeys {
    pub fn register(
        conn: &mut PgConnection,
        id: &str,
        author: &str,
        tagged: &str,
        relay: &str,
        name: &str,
    ) -> anyhow::Result<()> {
        let new = NewNwcPubkeys {
            id,
            author,
            tagged,
            relay,
            name,
        };

        diesel::insert_into(nwc_pubkeys::table)
            .values(&new)
            .execute(conn)?;

        Ok(())
    }

    pub fn get_filter_info(conn: &mut PgConnection) -> anyhow::Result<NwcFilterInfo> {
        conn.transaction(|conn| {
            let authors = nwc_pubkeys::table
                .select(nwc_pubkeys::author)
                .distinct()
                .load::<String>(conn)?;

            let tagged = nwc_pubkeys::table
                .select(nwc_pubkeys::tagged)
                .distinct()
                .load::<String>(conn)?
                .into_iter()
                .flat_map(|s| XOnlyPublicKey::from_str(&s).ok())
                .collect::<Vec<_>>();

            let relays = nwc_pubkeys::table
                .select(nwc_pubkeys::relay)
                .distinct()
                .load::<String>(conn)?;

            Ok(NwcFilterInfo {
                authors,
                tagged,
                relays,
            })
        })
    }
}
