-- Exclude p-gated Agent Runtime Payment ledger ciphertext from full-text search.
-- Additive migration: previously applied generated-column migrations remain frozen.

ALTER TABLE events DROP COLUMN search_tsv;
ALTER TABLE events ADD COLUMN search_tsv TSVECTOR GENERATED ALWAYS AS (
    CASE WHEN kind IN (1059, 30300, 30350, 30622, 44100, 44101, 44200, 44210, 44211, 44212)
         THEN NULL::tsvector
         ELSE to_tsvector('simple', content)
    END
) STORED;

CREATE INDEX idx_events_search_tsv ON events USING GIN (search_tsv);
