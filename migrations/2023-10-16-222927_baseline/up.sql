CREATE TABLE subscription_info
(
    id         TEXT PRIMARY KEY,
    endpoint   TEXT      NOT NULL,
    p256dh     TEXT      NOT NULL,
    auth       TEXT      NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE TABLE nwc_pubkeys
(
    id         TEXT      NOT NULL,    -- foreign key to subscription_info.id
    author     TEXT      NOT NULL,    -- the pubkey of which we expect to see as the author of the event
    tagged     TEXT      NOT NULL,    -- the pubkey of which we expect to see tagged in the event
    relay      TEXT      NOT NULL,    -- the relay for the nwc
    name       TEXT      NOT NULL,    -- the name the user gave to the nwc
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    PRIMARY KEY (id, author, tagged), -- composite primary key to prevent duplicates
    FOREIGN KEY (id) REFERENCES subscription_info (id)
);
