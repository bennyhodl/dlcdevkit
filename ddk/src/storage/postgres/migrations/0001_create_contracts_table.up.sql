CREATE TABLE contracts (
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
