-- BDK's changeset carries the wallet's locked outpoints (coins reserved for
-- in-flight DLC offers). They were silently dropped on persist, so a lock
-- did not survive a restart and a restarted wallet could select a reserved
-- coin a second time.
CREATE TABLE locked_outpoints (
    wallet_name TEXT NOT NULL,
    txid TEXT NOT NULL,
    vout INTEGER NOT NULL,
    is_locked BOOLEAN NOT NULL,
    PRIMARY KEY (wallet_name, txid, vout)
);
