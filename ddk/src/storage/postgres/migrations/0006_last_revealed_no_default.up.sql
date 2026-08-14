-- A fresh keychain row read back last_revealed = 0, which BDK interprets as
-- "derivation index 0 has been revealed", so the wallet skipped its first
-- address. NULL is the correct "nothing revealed yet" value. Existing rows at
-- 0 are ambiguous (genuinely revealed index 0, or never revealed) and are left
-- untouched; update_last_revealed only clamps upward, so the cost is at most
-- one skipped address on a wallet that never revealed anything.
ALTER TABLE keychain ALTER COLUMN last_revealed DROP DEFAULT;
