// @generated automatically by Diesel CLI.

diesel::table! {
    nwc_invoice_monitors (id, request_event_id, wallet_service_pubkey, relay) {
        id -> Text,
        request_event_id -> Text,
        client_pubkey -> Text,
        wallet_service_pubkey -> Text,
        relay -> Text,
        expires_at -> Timestamptz,
        next_wake_at -> Timestamptz,
        wake_count -> Int4,
        enabled -> Bool,
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
        trigger_token_hash -> Nullable<Text>,
        settlement_signaled_at -> Nullable<Timestamptz>,
        silent_sent_at -> Nullable<Timestamptz>,
        alert_sent_at -> Nullable<Timestamptz>,
    }
}

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
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
    }
}

diesel::table! {
    nwc_wake_events (event_id) {
        event_id -> Text,
        event_created_at -> Nullable<Int8>,
        received_at -> Timestamptz,
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
    nwc_invoice_monitors,
    nwc_push_registrations,
    nwc_wake_events,
    nwc_pubkeys,
    subscription_info,
);
