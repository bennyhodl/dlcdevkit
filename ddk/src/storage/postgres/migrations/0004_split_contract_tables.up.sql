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
    pnl BIGINT
);

CREATE INDEX idx_contract_metadata_state ON contract_metadata(state);

CREATE TABLE contract_data (
    id TEXT PRIMARY KEY REFERENCES contract_metadata(id) ON DELETE CASCADE,
    contract_data BYTEA NOT NULL,
    is_compressed BOOLEAN NOT NULL DEFAULT false
);

INSERT INTO contract_metadata (
    id, state, is_offer_party, counter_party,
    offer_collateral, accept_collateral, total_collateral, fee_rate_per_vb,
    cet_locktime, refund_locktime, pnl
)
SELECT 
    id, state, is_offer_party, counter_party,
    offer_collateral, accept_collateral, total_collateral, fee_rate_per_vb,
    cet_locktime, refund_locktime, pnl
FROM contracts;

INSERT INTO contract_data (id, contract_data, is_compressed)
SELECT id, contract_data, false FROM contracts;
