-- BDK's tx_graph changeset also carries first_seen (first time a tx was seen
-- in the mempool) and last_evicted (last time it was missing from the
-- mempool). Both were silently dropped on persist; losing last_evicted can
-- resurrect an RBF-replaced or evicted transaction as unconfirmed after a
-- restart.
ALTER TABLE tx ADD COLUMN first_seen BIGINT;
ALTER TABLE tx ADD COLUMN last_evicted BIGINT;

-- The keychain indexer changeset carries a cache of derived script pubkeys.
CREATE TABLE spk_cache (
    wallet_name TEXT NOT NULL,
    descriptor_id BYTEA NOT NULL,
    spk_index INTEGER NOT NULL,
    script BYTEA NOT NULL,
    PRIMARY KEY (wallet_name, descriptor_id, spk_index)
);
