-- Schema version control
CREATE TABLE IF NOT EXISTS version (
    version INTEGER PRIMARY KEY
);

-- Network is the valid network for all other table data
CREATE TABLE IF NOT EXISTS network (
    wallet_name TEXT PRIMARY KEY,
    name TEXT NOT NULL
);

-- Keychain is the json serialized keychain structure as JSONB,
-- descriptor is the complete descriptor string,
-- descriptor_id is a sha256::Hash id of the descriptor string w/o the checksum,
-- last revealed index is a u32
CREATE TABLE IF NOT EXISTS keychain (
    wallet_name TEXT NOT NULL,
    keychainkind TEXT NOT NULL,
    descriptor TEXT NOT NULL,
    descriptor_id BYTEA NOT NULL,
    last_revealed INTEGER DEFAULT 0,
    PRIMARY KEY (wallet_name, keychainkind)

);

-- Hash is block hash hex string,
-- Block height is a u32
CREATE TABLE IF NOT EXISTS block (
    wallet_name TEXT NOT NULL,
    hash TEXT NOT NULL,
    height INTEGER NOT NULL,
    PRIMARY KEY (wallet_name, hash)
);
CREATE INDEX idx_block_height ON block (height);

-- Txid is transaction hash hex string (reversed)
-- Whole_tx is a consensus encoded transaction,
-- Last seen is a u64 unix epoch seconds
CREATE TABLE IF NOT EXISTS tx (
    wallet_name TEXT NOT NULL,
    txid TEXT NOT NULL,
    whole_tx BYTEA,
    last_seen BIGINT,
    PRIMARY KEY (wallet_name, txid)
);

-- Outpoint txid hash hex string (reversed)
-- Outpoint vout
-- TxOut value as SATs
-- TxOut script consensus encoded
CREATE TABLE IF NOT EXISTS txout (
    wallet_name TEXT NOT NULL,
    txid TEXT NOT NULL,
    vout INTEGER NOT NULL,
    value BIGINT NOT NULL,
    script BYTEA NOT NULL,
    PRIMARY KEY (wallet_name, txid, vout)
);

-- Join table between anchor and tx
-- Block hash hex string
-- Anchor is a json serialized Anchor structure as JSONB,
-- Txid is transaction hash hex string (reversed)
CREATE TABLE IF NOT EXISTS anchor_tx (
    wallet_name TEXT NOT NULL,
    block_hash TEXT NOT NULL,
    anchor JSONB NOT NULL,
    txid TEXT NOT NULL,
    PRIMARY KEY (wallet_name, block_hash, txid),
    FOREIGN KEY (wallet_name, block_hash) REFERENCES block(wallet_name, hash),
    FOREIGN KEY (wallet_name, txid) REFERENCES tx(wallet_name, txid)
);
CREATE INDEX idx_anchor_tx_txid ON anchor_tx (txid);