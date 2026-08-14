-- The contract UTXO tracker watches the 2-of-2 funding outputs beside the
-- BDK wallet. Its changeset (funding script registrations and a transaction
-- graph changeset) is serde-serializable and monotone under merge, so one
-- merged JSONB document per wallet is enough.
CREATE TABLE contract_tracker (
    wallet_name TEXT PRIMARY KEY,
    changeset JSONB NOT NULL
);
