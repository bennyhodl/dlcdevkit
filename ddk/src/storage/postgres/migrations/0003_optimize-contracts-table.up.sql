-- Create new tables
CREATE TABLE contract_metadata (
    id TEXT PRIMARY KEY,
    state SMALLINT NOT NULL CHECK (state >= 0),
    is_offer_party BOOLEAN NOT NULL,
    counter_party TEXT NOT NULL,
    offer_collateral BIGINT NOT NULL CHECK (offer_collateral >= 0),
    accept_collateral BIGINT NOT NULL CHECK (accept_collateral >= 0),
    total_collateral BIGINT NOT NULL CHECK (total_collateral >= 0),
    fee_rate_per_vb BIGINT NOT NULL CHECK (fee_rate_per_vb >= 0),
    cet_locktime INTEGER NOT NULL CHECK (cet_locktime >= 0),
    refund_locktime INTEGER NOT NULL CHECK (refund_locktime >= 0),
    funding_txid TEXT,
    cet_txid TEXT,
    announcement_id TEXT NOT NULL DEFAULT 'legacy_data',
    oracle_pubkey TEXT NOT NULL DEFAULT 'legacy_data',
    pnl BIGINT
);

-- Add index on the state column which is used in frequent queries
CREATE INDEX idx_contract_metadata_state ON contract_metadata(state);
CREATE INDEX idx_contract_metadata_id ON contract_metadata(id);
CREATE INDEX idx_contract_metadata_counter_party ON contract_metadata(counter_party);

-- Create separate table for large binary data with an index on the id
CREATE TABLE contract_data (
    id TEXT PRIMARY KEY REFERENCES contract_metadata(id) ON DELETE CASCADE,
    state SMALLINT NOT NULL CHECK (state >= 0),
    contract_data BYTEA NOT NULL,
    is_compressed BOOLEAN NOT NULL DEFAULT false
);

-- Migrate existing data
INSERT INTO contract_metadata (
    id, state, is_offer_party, counter_party,
    offer_collateral, accept_collateral, total_collateral, fee_rate_per_vb,
    cet_locktime, refund_locktime, pnl, funding_txid, cet_txid, announcement_id, oracle_pubkey
)
SELECT 
    id, state, is_offer_party, counter_party,
    offer_collateral, accept_collateral, total_collateral, fee_rate_per_vb,
    cet_locktime, refund_locktime, pnl, 
    NULL as funding_txid, -- Adding NULL for new columns
    NULL as cet_txid,
    'legacy_data' as announcement_id, -- Default value for NOT NULL column
    'legacy_data' as oracle_pubkey
FROM contracts;

-- Copy binary data to the new table
INSERT INTO contract_data (id, state, contract_data, is_compressed)
SELECT id, state, contract_data, false FROM contracts;

-- Add index on contract_data.id
CREATE INDEX idx_contract_data_id ON contract_data(id);
CREATE INDEX idx_contract_data_state ON contract_data(state);
