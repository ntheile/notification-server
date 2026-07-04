// @generated automatically by Diesel CLI.

diesel::table! {
    nwc_push_registrations (id, push_service, author, tagged, relay) {
        id -> Text,
        push_service -> Text,
        push_token -> Text,
        app_id -> Text,
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
    nwc_wake_events (event_id) {
        event_id -> Text,
        event_created_at -> Nullable<Int8>,
        received_at -> Timestamp,
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
    nwc_push_registrations,
    nwc_wake_events,
    nwc_pubkeys,
    subscription_info,
);
