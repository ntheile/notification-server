CREATE TABLE nwc_invoice_monitors
(
    id                     TEXT        NOT NULL,
    request_event_id       TEXT        NOT NULL,
    client_pubkey          TEXT        NOT NULL,
    wallet_service_pubkey  TEXT        NOT NULL,
    relay                  TEXT        NOT NULL,
    expires_at             TIMESTAMPTZ NOT NULL,
    next_wake_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    wake_count             INTEGER     NOT NULL DEFAULT 0,
    enabled                BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT nwc_invoice_monitors_event_id_check
        CHECK (request_event_id ~ '^[0-9a-f]{64}$'),
    CONSTRAINT nwc_invoice_monitors_client_key_check
        CHECK (client_pubkey ~ '^[0-9a-f]{64}$'),
    CONSTRAINT nwc_invoice_monitors_wallet_key_check
        CHECK (wallet_service_pubkey ~ '^[0-9a-f]{64}$'),
    CONSTRAINT nwc_invoice_monitors_wake_count_check
        CHECK (wake_count BETWEEN 0 AND 64),
    PRIMARY KEY (id, request_event_id, wallet_service_pubkey, relay)
);

CREATE INDEX nwc_invoice_monitors_due_idx
    ON nwc_invoice_monitors (next_wake_at)
    WHERE enabled = TRUE;
