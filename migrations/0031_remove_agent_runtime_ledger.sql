-- Settled BOLT12 zaps now grant a five-minute invocation window. The paid
-- runtime ledger and its single-use reservation claim are no longer used.

DROP TRIGGER IF EXISTS trg_agent_runtime_reservation_claim ON events;
DROP FUNCTION IF EXISTS enforce_agent_runtime_reservation_claim();
DROP TABLE IF EXISTS agent_runtime_reservation_claims;
