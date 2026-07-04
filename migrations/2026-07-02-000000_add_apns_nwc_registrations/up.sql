CREATE TABLE nwc_push_registrations
(
    id           TEXT      NOT NULL,
    push_service TEXT      NOT NULL,
    push_token   TEXT      NOT NULL,
    app_id       TEXT      NOT NULL,
    environment  TEXT      NOT NULL DEFAULT 'production',
    author       TEXT      NOT NULL,
    tagged       TEXT      NOT NULL,
    relay        TEXT      NOT NULL,
    name         TEXT      NOT NULL,
    enabled      BOOLEAN   NOT NULL DEFAULT TRUE,
    created_at   TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMP NOT NULL DEFAULT NOW(),
    CONSTRAINT nwc_push_registrations_push_service_check CHECK (push_service IN ('apns', 'fcm')),
    CONSTRAINT nwc_push_registrations_environment_check CHECK (environment IN ('sandbox', 'production')),
    PRIMARY KEY (id, push_service, author, tagged, relay)
);

CREATE INDEX nwc_push_registrations_lookup_idx
    ON nwc_push_registrations (push_service, author, tagged, relay)
    WHERE enabled = TRUE;

CREATE TABLE nwc_wake_events
(
    event_id         TEXT      NOT NULL PRIMARY KEY,
    event_created_at BIGINT,
    received_at      TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX nwc_wake_events_received_at_idx
    ON nwc_wake_events (received_at);
