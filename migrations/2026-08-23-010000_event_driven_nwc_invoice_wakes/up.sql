ALTER TABLE nwc_invoice_monitors
    ADD COLUMN trigger_token_hash TEXT,
    ADD COLUMN settlement_signaled_at TIMESTAMPTZ,
    ADD COLUMN silent_sent_at TIMESTAMPTZ,
    ADD COLUMN alert_sent_at TIMESTAMPTZ,
    ADD CONSTRAINT nwc_invoice_monitors_trigger_hash_check
        CHECK (trigger_token_hash IS NULL OR trigger_token_hash ~ '^[0-9a-f]{64}$');

DROP INDEX nwc_invoice_monitors_due_idx;

CREATE INDEX nwc_invoice_monitors_due_idx
    ON nwc_invoice_monitors (next_wake_at)
    WHERE enabled = TRUE
      AND settlement_signaled_at IS NOT NULL
      AND alert_sent_at IS NULL;

CREATE INDEX nwc_invoice_monitors_trigger_idx
    ON nwc_invoice_monitors (request_event_id, trigger_token_hash)
    WHERE enabled = TRUE;
