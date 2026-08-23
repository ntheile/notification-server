DROP INDEX nwc_invoice_monitors_due_idx;
DROP INDEX nwc_invoice_monitors_trigger_idx;

ALTER TABLE nwc_invoice_monitors
    DROP CONSTRAINT nwc_invoice_monitors_trigger_hash_check,
    DROP COLUMN alert_sent_at,
    DROP COLUMN silent_sent_at,
    DROP COLUMN settlement_signaled_at,
    DROP COLUMN trigger_token_hash;

CREATE INDEX nwc_invoice_monitors_due_idx
    ON nwc_invoice_monitors (next_wake_at)
    WHERE enabled = TRUE;
