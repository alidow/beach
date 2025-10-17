CREATE TABLE IF NOT EXISTS controller_lease (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v7(),
    session_id UUID NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    controller_account_id UUID REFERENCES account(id),
    issued_by_account_id UUID REFERENCES account(id),
    reason TEXT,
    issued_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    UNIQUE (session_id)
);

CREATE TABLE IF NOT EXISTS session_runtime (
    session_id UUID PRIMARY KEY REFERENCES session(id) ON DELETE CASCADE,
    state_cache_url TEXT,
    transport_hints JSONB DEFAULT '{}'::jsonb,
    last_health JSONB,
    last_health_at TIMESTAMPTZ,
    last_state JSONB,
    last_state_at TIMESTAMPTZ
);

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'harness_type') THEN
        CREATE TYPE harness_type AS ENUM (
            'terminal_shim',
            'cabana_adapter',
            'remote_widget',
            'service_proxy',
            'custom'
        );
    END IF;
END
$$;

ALTER TABLE controller_event
ADD COLUMN IF NOT EXISTS controller_account_id UUID REFERENCES account(id),
ADD COLUMN IF NOT EXISTS issued_by_account_id UUID REFERENCES account(id),
ADD COLUMN IF NOT EXISTS controller_token_id UUID REFERENCES controller_lease(id);

ALTER TABLE session
ADD COLUMN IF NOT EXISTS harness_type harness_type;

CREATE INDEX IF NOT EXISTS idx_controller_lease_session ON controller_lease(session_id);

ALTER TABLE controller_lease ENABLE ROW LEVEL SECURITY;
ALTER TABLE session_runtime ENABLE ROW LEVEL SECURITY;
