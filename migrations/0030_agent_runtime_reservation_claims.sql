-- Atomically bind one paid-runtime reservation to at most one instruction.
-- The relay event insert and this state transition share one transaction, so
-- independent ACP processes cannot consume the same reservation concurrently.

CREATE TABLE agent_runtime_reservation_claims (
    community_id UUID NOT NULL REFERENCES communities(id),
    reservation_event_id BYTEA NOT NULL,
    agent_pubkey BYTEA NOT NULL,
    payer_pubkey BYTEA NOT NULL,
    channel_id UUID NOT NULL,
    must_start_by BIGINT NOT NULL,
    instruction_event_id BYTEA,
    claimed_at TIMESTAMPTZ,
    settled BOOLEAN NOT NULL DEFAULT FALSE,
    settled_at TIMESTAMPTZ,
    PRIMARY KEY (community_id, reservation_event_id),
    CONSTRAINT chk_runtime_claim_reservation_id_len
        CHECK (length(reservation_event_id) = 32),
    CONSTRAINT chk_runtime_claim_agent_pubkey_len CHECK (length(agent_pubkey) = 32),
    CONSTRAINT chk_runtime_claim_payer_pubkey_len CHECK (length(payer_pubkey) = 32),
    CONSTRAINT chk_runtime_claim_instruction_id_len
        CHECK (instruction_event_id IS NULL OR length(instruction_event_id) = 32)
);

CREATE INDEX idx_runtime_claims_payer_channel
    ON agent_runtime_reservation_claims
    (community_id, payer_pubkey, channel_id, settled);

CREATE OR REPLACE FUNCTION enforce_agent_runtime_reservation_claim()
RETURNS TRIGGER AS $$
DECLARE
    tag JSONB;
    tag_count INTEGER;
    payer_hex TEXT;
    channel_text TEXT;
    expiration_text TEXT;
    reservation_hex TEXT;
    agent_hex TEXT;
    changed INTEGER;
BEGIN
    IF NEW.kind = 44211 THEN
        SELECT count(*), min(value->>1)
          INTO tag_count, payer_hex
          FROM jsonb_array_elements(NEW.tags) value
         WHERE jsonb_typeof(value) = 'array'
           AND jsonb_array_length(value) = 2
           AND value->>0 = 'p';
        IF tag_count <> 1 OR payer_hex !~ '^[0-9a-f]{64}$' THEN
            RAISE EXCEPTION 'agent_runtime_claim: reservation requires one canonical p tag'
                USING ERRCODE = '23514';
        END IF;

        SELECT count(*), min(value->>1)
          INTO tag_count, channel_text
          FROM jsonb_array_elements(NEW.tags) value
         WHERE jsonb_typeof(value) = 'array'
           AND jsonb_array_length(value) = 2
           AND value->>0 = 'h';
        IF tag_count <> 1 OR channel_text !~
            '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$' THEN
            RAISE EXCEPTION 'agent_runtime_claim: reservation channel is invalid'
                USING ERRCODE = '23514';
        END IF;

        SELECT count(*), min(value->>1)
          INTO tag_count, expiration_text
          FROM jsonb_array_elements(NEW.tags) value
         WHERE jsonb_typeof(value) = 'array'
           AND jsonb_array_length(value) = 2
           AND value->>0 = 'expiration';
        IF tag_count <> 1 OR expiration_text !~ '^[0-9]+$' THEN
            RAISE EXCEPTION 'agent_runtime_claim: reservation expiration is invalid'
                USING ERRCODE = '23514';
        END IF;

        INSERT INTO agent_runtime_reservation_claims (
            community_id, reservation_event_id, agent_pubkey, payer_pubkey,
            channel_id, must_start_by
        ) VALUES (
            NEW.community_id, NEW.id, NEW.pubkey, decode(payer_hex, 'hex'),
            channel_text::uuid, expiration_text::bigint
        )
        ON CONFLICT (community_id, reservation_event_id) DO NOTHING;
    END IF;

    IF NEW.kind = 44212 THEN
        SELECT count(*), min(value->>1)
          INTO tag_count, reservation_hex
          FROM jsonb_array_elements(NEW.tags) value
         WHERE jsonb_typeof(value) = 'array'
           AND jsonb_array_length(value) = 2
           AND value->>0 = 'e';
        IF tag_count <> 1 OR reservation_hex !~ '^[0-9a-f]{64}$' THEN
            RAISE EXCEPTION 'agent_runtime_claim: settlement requires one reservation reference'
                USING ERRCODE = '23514';
        END IF;

        SELECT count(*), min(value->>1)
          INTO tag_count, payer_hex
          FROM jsonb_array_elements(NEW.tags) value
         WHERE jsonb_typeof(value) = 'array'
           AND jsonb_array_length(value) = 2
           AND value->>0 = 'p';
        IF tag_count <> 1 OR payer_hex !~ '^[0-9a-f]{64}$' THEN
            RAISE EXCEPTION 'agent_runtime_claim: settlement requires one canonical p tag'
                USING ERRCODE = '23514';
        END IF;

        SELECT count(*), min(value->>1)
          INTO tag_count, channel_text
          FROM jsonb_array_elements(NEW.tags) value
         WHERE jsonb_typeof(value) = 'array'
           AND jsonb_array_length(value) = 2
           AND value->>0 = 'h';
        IF tag_count <> 1 OR channel_text !~
            '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$' THEN
            RAISE EXCEPTION 'agent_runtime_claim: settlement channel is invalid'
                USING ERRCODE = '23514';
        END IF;

        UPDATE agent_runtime_reservation_claims
           SET settled = TRUE, settled_at = COALESCE(settled_at, NOW())
         WHERE community_id = NEW.community_id
           AND reservation_event_id = decode(reservation_hex, 'hex')
           AND agent_pubkey = NEW.pubkey
           AND payer_pubkey = decode(payer_hex, 'hex')
           AND channel_id = channel_text::uuid;
        GET DIAGNOSTICS changed = ROW_COUNT;
        IF changed <> 1 THEN
            RAISE EXCEPTION 'agent_runtime_claim: settlement reservation mismatch'
                USING ERRCODE = '23514';
        END IF;
    END IF;

    FOR tag IN
        SELECT value
          FROM jsonb_array_elements(NEW.tags) value
         WHERE jsonb_typeof(value) = 'array' AND value->>0 = 'agent_runtime'
    LOOP
        IF jsonb_array_length(tag) <> 3 THEN
            RAISE EXCEPTION 'agent_runtime_claim: malformed instruction tag'
                USING ERRCODE = '23514';
        END IF;
        agent_hex := tag->>1;
        reservation_hex := tag->>2;
        IF agent_hex !~ '^[0-9a-f]{64}$' OR reservation_hex !~ '^[0-9a-f]{64}$'
           OR NEW.channel_id IS NULL THEN
            RAISE EXCEPTION 'agent_runtime_claim: invalid instruction routing'
                USING ERRCODE = '23514';
        END IF;

        UPDATE agent_runtime_reservation_claims
           SET instruction_event_id = NEW.id,
               claimed_at = COALESCE(claimed_at, NOW())
         WHERE community_id = NEW.community_id
           AND reservation_event_id = decode(reservation_hex, 'hex')
           AND agent_pubkey = decode(agent_hex, 'hex')
           AND payer_pubkey = NEW.pubkey
           AND channel_id = NEW.channel_id
           AND must_start_by >= extract(epoch FROM NOW())::bigint
           AND NOT settled
           AND (instruction_event_id IS NULL OR instruction_event_id = NEW.id);
        GET DIAGNOSTICS changed = ROW_COUNT;
        IF changed <> 1 THEN
            RAISE EXCEPTION 'agent_runtime_claim: reservation is unavailable or already consumed'
                USING ERRCODE = '23514';
        END IF;
    END LOOP;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_agent_runtime_reservation_claim
    BEFORE INSERT ON events
    FOR EACH ROW EXECUTE FUNCTION enforce_agent_runtime_reservation_claim();
