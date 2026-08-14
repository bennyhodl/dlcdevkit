-- BIP-329 wallet labels. The key is the record type tag plus the
-- reference, because an input record and an output record can share the
-- same outpoint reference.
CREATE TABLE wallet_labels (
    wallet_name TEXT NOT NULL,
    label_key TEXT NOT NULL,
    label JSONB NOT NULL,
    PRIMARY KEY (wallet_name, label_key)
);
