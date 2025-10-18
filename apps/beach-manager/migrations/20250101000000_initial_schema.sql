CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS citext;
CREATE TYPE account_type AS ENUM ('human', 'agent', 'service');
CREATE TYPE account_status AS ENUM ('active', 'disabled');
CREATE TYPE membership_role AS ENUM ('owner', 'admin', 'contributor', 'viewer');
CREATE TYPE membership_status AS ENUM ('active', 'invited', 'suspended', 'revoked');
CREATE TYPE group_role AS ENUM ('admin', 'member');
CREATE TYPE organization_role AS ENUM ('owner', 'admin', 'billing', 'member');
CREATE TYPE session_kind AS ENUM ('terminal', 'cabana_gui', 'manager_console', 'widget', 'spectator_feed', 'service_daemon');
CREATE TYPE automation_role AS ENUM ('observer', 'controller', 'coordinator');
CREATE TYPE controller_event_type AS ENUM ('registered', 'lease_acquired', 'lease_released', 'actions_queued', 'actions_acked', 'health_reported', 'state_updated');

CREATE TABLE organization (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name TEXT NOT NULL,
    slug CITEXT UNIQUE NOT NULL,
    billing_email CITEXT,
    metadata JSONB DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    archived_at TIMESTAMPTZ
);

CREATE TABLE account (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    type account_type NOT NULL,
    status account_status NOT NULL DEFAULT 'active',
    beach_gate_subject TEXT UNIQUE NOT NULL,
    display_name TEXT,
    email CITEXT UNIQUE,
    avatar_url TEXT,
    metadata JSONB DEFAULT '{}'::jsonb,
    default_organization_id UUID REFERENCES organization(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE organization_membership (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    organization_id UUID NOT NULL REFERENCES organization(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    role organization_role NOT NULL,
    created_by_account_id UUID REFERENCES account(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (organization_id, account_id)
);

CREATE TABLE private_beach (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    organization_id UUID REFERENCES organization(id) ON DELETE CASCADE,
    owner_account_id UUID REFERENCES account(id),
    name TEXT NOT NULL,
    slug CITEXT UNIQUE NOT NULL,
    description TEXT,
    default_role membership_role NOT NULL DEFAULT 'viewer',
    layout_preset JSONB DEFAULT '{}'::jsonb,
    settings JSONB DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    archived_at TIMESTAMPTZ
);

CREATE TABLE private_beach_membership (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    private_beach_id UUID NOT NULL REFERENCES private_beach(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    role membership_role NOT NULL,
    status membership_status NOT NULL DEFAULT 'active',
    invited_by_account_id UUID REFERENCES account(id),
    invitation_token_hash TEXT,
    invited_at TIMESTAMPTZ,
    activated_at TIMESTAMPTZ,
    suspended_at TIMESTAMPTZ,
    notes TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (private_beach_id, account_id)
);

CREATE TABLE beach_group (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    private_beach_id UUID NOT NULL REFERENCES private_beach(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT,
    created_by_account_id UUID REFERENCES account(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Enforce case-insensitive uniqueness for group name within a beach
CREATE UNIQUE INDEX IF NOT EXISTS idx_beach_group_name_ci
  ON beach_group (private_beach_id, lower(name));

CREATE TABLE group_membership (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    beach_group_id UUID NOT NULL REFERENCES beach_group(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    role group_role NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (beach_group_id, account_id)
);

CREATE TABLE share_link (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    private_beach_id UUID NOT NULL REFERENCES private_beach(id) ON DELETE CASCADE,
    created_by_account_id UUID REFERENCES account(id),
    label TEXT,
    token_hash TEXT UNIQUE NOT NULL,
    granted_role membership_role NOT NULL,
    max_uses INTEGER,
    use_count INTEGER NOT NULL DEFAULT 0,
    expires_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE session (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    private_beach_id UUID NOT NULL REFERENCES private_beach(id) ON DELETE CASCADE,
    origin_session_id UUID NOT NULL,
    harness_id UUID,
    kind session_kind NOT NULL,
    title TEXT,
    display_order INTEGER,
    location_hint TEXT,
    capabilities JSONB DEFAULT '[]'::jsonb,
    metadata JSONB DEFAULT '{}'::jsonb,
    created_by_account_id UUID REFERENCES account(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at TIMESTAMPTZ,
    ended_at TIMESTAMPTZ,
    UNIQUE (private_beach_id, origin_session_id)
);

CREATE TABLE session_tag (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    session_id UUID NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    tag TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Enforce case-insensitive uniqueness for tags per session
CREATE UNIQUE INDEX IF NOT EXISTS idx_session_tag_ci
  ON session_tag (session_id, lower(tag));

CREATE TABLE automation_assignment (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    private_beach_id UUID NOT NULL REFERENCES private_beach(id) ON DELETE CASCADE,
    controller_account_id UUID NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    role automation_role NOT NULL,
    session_id UUID REFERENCES session(id) ON DELETE CASCADE,
    config JSONB DEFAULT '{}'::jsonb,
    created_by_account_id UUID REFERENCES account(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX automation_assignment_unique_session
ON automation_assignment(controller_account_id, session_id)
WHERE session_id IS NOT NULL;

CREATE TABLE controller_event (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    session_id UUID NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    event_type controller_event_type NOT NULL,
    controller_token UUID,
    reason TEXT,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE file_record (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    private_beach_id UUID NOT NULL REFERENCES private_beach(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    storage_key TEXT NOT NULL,
    size_bytes BIGINT,
    content_type TEXT,
    checksum TEXT,
    uploaded_by_account_id UUID REFERENCES account(id),
    uploaded_by_session_id UUID REFERENCES session(id),
    uploaded_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at TIMESTAMPTZ,
    UNIQUE (private_beach_id, path, version)
);

CREATE INDEX idx_private_beach_membership_account ON private_beach_membership(account_id);
CREATE INDEX idx_session_private_beach ON session(private_beach_id);
CREATE INDEX idx_controller_event_session ON controller_event(session_id, occurred_at DESC);

ALTER TABLE session ENABLE ROW LEVEL SECURITY;
ALTER TABLE private_beach ENABLE ROW LEVEL SECURITY;
ALTER TABLE private_beach_membership ENABLE ROW LEVEL SECURITY;
ALTER TABLE automation_assignment ENABLE ROW LEVEL SECURITY;
ALTER TABLE controller_event ENABLE ROW LEVEL SECURITY;
