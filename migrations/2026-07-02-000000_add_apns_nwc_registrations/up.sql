CREATE TABLE apns_nwc_registrations
(
    id           TEXT      NOT NULL,
    device_token TEXT      NOT NULL,
    bundle_id    TEXT      NOT NULL,
    environment  TEXT      NOT NULL DEFAULT 'sandbox',
    author       TEXT      NOT NULL,
    tagged       TEXT      NOT NULL,
    relay        TEXT      NOT NULL,
    name         TEXT      NOT NULL,
    enabled      BOOLEAN   NOT NULL DEFAULT TRUE,
    created_at   TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMP NOT NULL DEFAULT NOW(),
    PRIMARY KEY (id, author, tagged, relay)
);

CREATE INDEX apns_nwc_registrations_lookup_idx
    ON apns_nwc_registrations (author, tagged, relay)
    WHERE enabled = TRUE;
