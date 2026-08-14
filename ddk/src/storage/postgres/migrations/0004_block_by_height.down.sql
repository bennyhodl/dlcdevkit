ALTER TABLE block DROP CONSTRAINT block_pkey;
ALTER TABLE block ADD PRIMARY KEY (wallet_name, hash);
CREATE INDEX idx_block_height ON block (height);
-- NOT VALID: anchors may reference block rows that no longer exist.
ALTER TABLE anchor_tx
    ADD FOREIGN KEY (wallet_name, block_hash)
    REFERENCES block (wallet_name, hash) NOT VALID;
