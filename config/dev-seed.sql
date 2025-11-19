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

-- Host CLI profile used by pong-stack.sh / harness
INSERT INTO account (id, type, status, beach_gate_subject, display_name, email, default_organization_id)
VALUES (
  '00000000-0000-0000-0000-000000000001', 'human', 'active', '00000000-0000-0000-0000-000000000001',
  'Host User', 'mock-user@beach.test', 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'
)
ON CONFLICT (id) DO NOTHING;

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
-- No sessions are seeded. Attach only real session IDs at runtime.

-- Ensure basic rows are present without raising errors if re-run
SELECT 1;
