-- Add down migration script here
-- DOWN migration to revert changes

-- First, ensure we have a backup of all data
CREATE TABLE IF NOT EXISTS contracts_backup AS 
SELECT 
    m.id, 
    m.state, 
    m.is_offer_party, 
    m.counter_party,
    m.offer_collateral, 
    m.accept_collateral, 
    m.total_collateral, 
    m.fee_rate_per_vb,
    m.cet_locktime, 
    m.refund_locktime, 
    m.pnl,
    d.contract_data
FROM contract_metadata m
JOIN contract_data d ON m.id = d.id;

-- Recreate the original contracts table if it doesn't exist
CREATE TABLE IF NOT EXISTS contracts (
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
    pnl BIGINT,
    contract_data BYTEA NOT NULL
);

-- Delete any existing data in the original table
DELETE FROM contracts;

-- Restore data from our backup to the contracts table
INSERT INTO contracts (
    id, state, is_offer_party, counter_party,
    offer_collateral, accept_collateral, total_collateral, fee_rate_per_vb,
    cet_locktime, refund_locktime, pnl, contract_data
)
SELECT 
    id, state, is_offer_party, counter_party,
    offer_collateral, accept_collateral, total_collateral, fee_rate_per_vb,
    cet_locktime, refund_locktime, pnl, contract_data
FROM contracts_backup;

-- Drop the backup table
DROP TABLE contracts_backup;

-- Drop the new tables with CASCADE to ensure indexes and constraints are also dropped
DROP TABLE IF EXISTS contract_data CASCADE;
DROP TABLE IF EXISTS contract_metadata CASCADE;

-- Just in case, explicitly drop the indexes (though CASCADE should handle this)
DROP INDEX IF EXISTS idx_contract_data_id;
DROP INDEX IF EXISTS idx_contract_metadata_state;