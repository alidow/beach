DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'controller_update_cadence') THEN
        CREATE TYPE controller_update_cadence AS ENUM ('fast', 'balanced', 'slow');
    END IF;
END
$$;

CREATE TABLE IF NOT EXISTS controller_pairing (
    controller_session_id UUID NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    child_session_id UUID NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    prompt_template TEXT,
    update_cadence controller_update_cadence NOT NULL DEFAULT 'balanced',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (controller_session_id, child_session_id)
);

CREATE INDEX IF NOT EXISTS idx_controller_pairing_child
    ON controller_pairing(child_session_id);

ALTER TABLE controller_pairing ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS controller_pairing_all ON controller_pairing;
CREATE POLICY controller_pairing_all ON controller_pairing
USING (
    EXISTS (
        SELECT 1
        FROM session s
        WHERE s.id = controller_pairing.controller_session_id
          AND s.private_beach_id::text = current_setting('beach.private_beach_id', true)
    )
    AND EXISTS (
        SELECT 1
        FROM session s
        WHERE s.id = controller_pairing.child_session_id
          AND s.private_beach_id::text = current_setting('beach.private_beach_id', true)
    )
)
WITH CHECK (
    EXISTS (
        SELECT 1
        FROM session s
        WHERE s.id = controller_pairing.controller_session_id
          AND s.private_beach_id::text = current_setting('beach.private_beach_id', true)
    )
    AND EXISTS (
        SELECT 1
        FROM session s
        WHERE s.id = controller_pairing.child_session_id
          AND s.private_beach_id::text = current_setting('beach.private_beach_id', true)
    )
);

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_type t
        JOIN pg_enum e ON t.oid = e.enumtypid
        WHERE t.typname = 'controller_event_type'
          AND e.enumlabel = 'pairing_added'
    ) THEN
        ALTER TYPE controller_event_type ADD VALUE 'pairing_added';
    END IF;

    IF NOT EXISTS (
        SELECT 1
        FROM pg_type t
        JOIN pg_enum e ON t.oid = e.enumtypid
        WHERE t.typname = 'controller_event_type'
          AND e.enumlabel = 'pairing_removed'
    ) THEN
        ALTER TYPE controller_event_type ADD VALUE 'pairing_removed';
    END IF;
END
$$;
