-- Claim each settled BOLT12 payment once per community.
--
-- A kind 9736 event uses the normal Nostr creation time. Different outer
-- events can therefore contain the same valid payer proof. This table makes
-- the proof's payment hash the durable relay-side idempotency key.
SET LOCAL lock_timeout = '5s';

CREATE TABLE bolt12_zap_payments (
    community_id UUID NOT NULL REFERENCES communities(id),
    payment_hash BYTEA NOT NULL CHECK (length(payment_hash) = 32),
    event_id     BYTEA NOT NULL CHECK (length(event_id) = 32),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (community_id, payment_hash),
    UNIQUE (community_id, event_id)
);

SELECT attach_community_write_fence('bolt12_zap_payments');
