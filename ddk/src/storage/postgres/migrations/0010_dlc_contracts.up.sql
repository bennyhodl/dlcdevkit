-- One row per contract, with the wire messages and the manager-only state in
-- their own columns. Replaces the opaque blob in contract_data and the
-- duplicated columns in contract_metadata. Rows are moved here by
-- `PostgresStore::migrate_legacy_contracts`, which needs the Rust decoder for
-- the old blob, so this migration only creates the table.
--
-- format_version: the layout of the row. 1 is the legacy blob layout that
-- lives in contract_data and never appears in this table. 2 is this layout.
CREATE TABLE dlc_contracts (
    id TEXT PRIMARY KEY,
    format_version SMALLINT NOT NULL,
    state SMALLINT NOT NULL CHECK (state >= 0),
    temporary_id TEXT NOT NULL,
    is_offer_party BOOLEAN NOT NULL,
    counter_party TEXT NOT NULL,
    keys_id BYTEA NOT NULL,
    contract_flags SMALLINT NOT NULL CHECK (contract_flags >= 0),
    chain_hash BYTEA,
    offer_collateral BIGINT NOT NULL CHECK (offer_collateral >= 0),
    accept_collateral BIGINT NOT NULL CHECK (accept_collateral >= 0),
    total_collateral BIGINT NOT NULL CHECK (total_collateral >= 0),
    fee_rate_per_vb BIGINT NOT NULL CHECK (fee_rate_per_vb >= 0),
    cet_locktime INTEGER NOT NULL CHECK (cet_locktime >= 0),
    refund_locktime INTEGER NOT NULL CHECK (refund_locktime >= 0),
    announcement_id TEXT NOT NULL,
    oracle_pubkey TEXT NOT NULL,
    funding_txid TEXT,
    cet_txid TEXT,
    pnl BIGINT,
    -- The DLC wire messages, byte for byte, including their TLV streams.
    offer_message BYTEA NOT NULL,
    accept_message BYTEA,
    sign_message BYTEA,
    -- Manager-only state that is not carried by the messages. Each column
    -- holds exactly one type with its own wire encoding.
    offer_params BYTEA NOT NULL,
    accept_params BYTEA,
    adaptor_infos BYTEA,
    dlc_transactions BYTEA,
    channel_id BYTEA,
    attestations BYTEA,
    signed_cet BYTEA,
    error_message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_dlc_contracts_state ON dlc_contracts (state);
CREATE INDEX idx_dlc_contracts_counter_party ON dlc_contracts (counter_party);
CREATE INDEX idx_dlc_contracts_temporary_id ON dlc_contracts (temporary_id);
