-- Add membership-based SELECT access to private_beach and allow viewing owner-owned beaches.
-- Also split write policies to use the per-beach GUC for INSERT/UPDATE/DELETE.

-- PRIVATE BEACH policies
DROP POLICY IF EXISTS private_beach_all ON private_beach;

-- SELECT allowed when caller is a member (active) or the owner. Also allow
-- dev bypass when owner_account_id IS NULL and the request-scoped beach.private_beach_id matches.
CREATE POLICY private_beach_member_select ON private_beach
FOR SELECT
USING (
  (
    EXISTS (
      SELECT 1 FROM private_beach_membership m
      WHERE m.private_beach_id = private_beach.id
        AND m.account_id::text = current_setting('beach.account_id', true)
        AND m.status = 'active'
    )
    OR owner_account_id::text = current_setting('beach.account_id', true)
  )
  OR (
    owner_account_id IS NULL
    AND id::text = current_setting('beach.private_beach_id', true)
  )
);

-- INSERT restricted by per-beach GUC
CREATE POLICY private_beach_insert ON private_beach
FOR INSERT
WITH CHECK (
  id::text = current_setting('beach.private_beach_id', true)
);

-- UPDATE restricted by per-beach GUC
CREATE POLICY private_beach_update ON private_beach
FOR UPDATE
USING (
  id::text = current_setting('beach.private_beach_id', true)
)
WITH CHECK (
  id::text = current_setting('beach.private_beach_id', true)
);

-- DELETE restricted by per-beach GUC
CREATE POLICY private_beach_delete ON private_beach
FOR DELETE
USING (
  id::text = current_setting('beach.private_beach_id', true)
);

-- MEMBERSHIP visibility (optional: needed by list code or admin tooling)
DROP POLICY IF EXISTS private_beach_membership_select ON private_beach_membership;
CREATE POLICY private_beach_membership_select ON private_beach_membership
FOR SELECT
USING (
  account_id::text = current_setting('beach.account_id', true)
);

-- Allow inserting owner membership when creating beach
DROP POLICY IF EXISTS private_beach_membership_insert ON private_beach_membership;
CREATE POLICY private_beach_membership_insert ON private_beach_membership
FOR INSERT
WITH CHECK (
  account_id::text = current_setting('beach.account_id', true)
  AND private_beach_id::text = current_setting('beach.private_beach_id', true)
);

