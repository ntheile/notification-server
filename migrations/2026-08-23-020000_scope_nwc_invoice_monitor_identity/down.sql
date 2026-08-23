ALTER TABLE nwc_invoice_monitors
    DROP CONSTRAINT IF EXISTS nwc_invoice_monitors_pkey;

ALTER TABLE nwc_invoice_monitors
    ADD PRIMARY KEY (id, request_event_id, relay);
