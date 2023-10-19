// @generated automatically by Diesel CLI.

diesel::table! {
    nwc_pubkeys (id, author, tagged) {
        id -> Text,
        author -> Text,
        tagged -> Text,
        relay -> Text,
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
    nwc_pubkeys,
    subscription_info,
);
