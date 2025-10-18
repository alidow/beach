-- Dev seed for Private Beach Manager
-- Creates a dev org, account, private beach, memberships, and a couple of sessions.

-- IDs are deterministic for easy reference
-- org:       aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa
-- account:   bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb
-- beach:     11111111-1111-1111-1111-111111111111 (slug: dev-beach)
-- sessions:  22222222-2222-2222-2222-222222222222 (terminal)
--            33333333-3333-3333-3333-333333333333 (cabana)

INSERT INTO organization (id, name, slug)
VALUES ('aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa', 'Dev Org', 'dev-org')
ON CONFLICT (slug) DO NOTHING;

INSERT INTO account (id, type, status, beach_gate_subject, display_name, email, default_organization_id)
VALUES (
  'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb', 'human', 'active', 'dev-subject', 'Dev User', 'dev@local', 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'
)
ON CONFLICT (beach_gate_subject) DO NOTHING;

INSERT INTO organization_membership (organization_id, account_id, role)
VALUES ('aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa', 'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb', 'owner')
ON CONFLICT (organization_id, account_id) DO NOTHING;

INSERT INTO private_beach (id, organization_id, owner_account_id, name, slug, description, default_role)
VALUES (
  '11111111-1111-1111-1111-111111111111', 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa', 'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb',
  'Dev Beach', 'dev-beach', 'Seeded dev private beach', 'contributor'
)
ON CONFLICT (slug) DO NOTHING;

INSERT INTO private_beach_membership (private_beach_id, account_id, role, status)
VALUES ('11111111-1111-1111-1111-111111111111', 'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb', 'owner', 'active')
ON CONFLICT (private_beach_id, account_id) DO NOTHING;

-- Seed two sessions for the dev beach
INSERT INTO session (id, private_beach_id, origin_session_id, harness_id, kind, title, location_hint, capabilities, metadata, created_by_account_id, created_at)
VALUES (
  '2f2f2f2f-2222-4222-8222-2f2f2f2f2f2f', '11111111-1111-1111-1111-111111111111', '22222222-2222-2222-2222-222222222222',
  '4a4a4a4a-aaaa-4aaa-8aaa-4a4a4a4a4a4a', 'terminal', 'Dev Terminal', 'local', '[]'::jsonb, '{}'::jsonb,
  'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb', NOW()
)
ON CONFLICT (private_beach_id, origin_session_id) DO NOTHING;

UPDATE session SET harness_type = 'terminal_shim'::harness_type WHERE origin_session_id = '22222222-2222-2222-2222-222222222222';

INSERT INTO session (id, private_beach_id, origin_session_id, harness_id, kind, title, location_hint, capabilities, metadata, created_by_account_id, created_at)
VALUES (
  '3f3f3f3f-3333-4333-8333-3f3f3f3f3f3f', '11111111-1111-1111-1111-111111111111', '33333333-3333-3333-3333-333333333333',
  '5b5b5b5b-bbbb-4bbb-8bbb-5b5b5b5b5b5b', 'cabana_gui', 'Dev Cabana', 'local', '[]'::jsonb, '{}'::jsonb,
  'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb', NOW()
)
ON CONFLICT (private_beach_id, origin_session_id) DO NOTHING;

UPDATE session SET harness_type = 'cabana_adapter'::harness_type WHERE origin_session_id = '33333333-3333-3333-3333-333333333333';

-- Optional: runtime rows (empty health/state)
INSERT INTO session_runtime (session_id, transport_hints)
SELECT id, '{}'::jsonb FROM session s
WHERE s.private_beach_id = '11111111-1111-1111-1111-111111111111'
ON CONFLICT (session_id) DO NOTHING;

-- Ensure basic rows are present without raising errors if re-run
SELECT 1;

