-- BDK's local chain is a map of height -> block hash: a reorg REPLACES the
-- hash at a height. Keying blocks by (wallet_name, hash) let the old and the
-- new hash accumulate at the same height, and the reader picked one of them
-- nondeterministically. Re-key the table by (wallet_name, height).

-- Anchors carry their full anchor block inside the JSONB payload and may
-- legitimately reference blocks that are no longer part of the sparse local
-- chain after a reorg, so anchor_tx must not require a matching block row;
-- the constraint also made reorg deletes of block rows fail and roll back
-- the whole persist transaction.
DO $$
DECLARE r RECORD;
BEGIN
    FOR r IN
        SELECT conname
        FROM pg_constraint
        WHERE conrelid = 'anchor_tx'::regclass
          AND contype = 'f'
          AND confrelid = 'block'::regclass
    LOOP
        EXECUTE format('ALTER TABLE anchor_tx DROP CONSTRAINT %I', r.conname);
    END LOOP;
END $$;

-- Deduplicate reorg leftovers. Prefer the hash an anchor still references;
-- tie-break on the greater hash. The next wallet sync repairs a wrong pick.
DELETE FROM block b
WHERE NOT EXISTS (
        SELECT 1 FROM anchor_tx a
        WHERE a.wallet_name = b.wallet_name AND a.block_hash = b.hash)
  AND EXISTS (
        SELECT 1
        FROM block b2
        JOIN anchor_tx a2
          ON a2.wallet_name = b2.wallet_name AND a2.block_hash = b2.hash
        WHERE b2.wallet_name = b.wallet_name
          AND b2.height = b.height
          AND b2.hash <> b.hash);

DELETE FROM block b
WHERE EXISTS (
        SELECT 1 FROM block b2
        WHERE b2.wallet_name = b.wallet_name
          AND b2.height = b.height
          AND b2.hash > b.hash);

ALTER TABLE block DROP CONSTRAINT block_pkey;
ALTER TABLE block ADD PRIMARY KEY (wallet_name, height);

-- Redundant now: the primary key covers (wallet_name, height) and every query
-- filters on wallet_name first.
DROP INDEX IF EXISTS idx_block_height;
