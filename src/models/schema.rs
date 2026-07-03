// @generated automatically by Diesel CLI.

diesel::table! {
    apns_nwc_registrations (id, author, tagged, relay) {
        id -> Text,
        device_token -> Text,
        bundle_id -> Text,
        environment -> Text,
        author -> Text,
        tagged -> Text,
        relay -> Text,
        name -> Text,
        enabled -> Bool,
        created_at -> Timestamp,
        updated_at -> Timestamp,
    }
}

diesel::table! {
    nwc_pubkeys (id, author, tagged) {
        id -> Text,
        author -> Text,
        tagged -> Text,
        relay -> Text,
        name -> Text,
        created_at -> Timestamp,
    }
}

diesel::table! {
    subscription_info (id) {
        id -> Text,
        endpoint -> Text,
        p256dh -> Text,
        auth -> Text,
        created_at -> Timestamp,
    }
}

diesel::joinable!(nwc_pubkeys -> subscription_info (id));

diesel::allow_tables_to_appear_in_same_query!(
    apns_nwc_registrations,
    nwc_pubkeys,
    subscription_info,
);
