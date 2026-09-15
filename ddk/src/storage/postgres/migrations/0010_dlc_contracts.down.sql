-- The legacy blob cannot be rebuilt in SQL, so a table that still holds
-- contracts must not be dropped. Delete or export the rows first.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM dlc_contracts) THEN
        RAISE EXCEPTION 'dlc_contracts still holds contracts; reverting would strand them';
    END IF;
END $$;

DROP TABLE dlc_contracts;
