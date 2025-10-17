-- RLS policies enforcing per-private-beach access using a request-scoped GUC.
-- The application sets: SELECT set_config('beach.private_beach_id', '<uuid>', true);

-- SESSION
DROP POLICY IF EXISTS session_select ON session;
DROP POLICY IF EXISTS session_insert ON session;
DROP POLICY IF EXISTS session_update ON session;
DROP POLICY IF EXISTS session_delete ON session;

CREATE POLICY session_select ON session
FOR SELECT
USING (
  private_beach_id::text = current_setting('beach.private_beach_id', true)
);

CREATE POLICY session_insert ON session
FOR INSERT
WITH CHECK (
  private_beach_id::text = current_setting('beach.private_beach_id', true)
);

CREATE POLICY session_update ON session
FOR UPDATE
USING (
  private_beach_id::text = current_setting('beach.private_beach_id', true)
)
WITH CHECK (
  private_beach_id::text = current_setting('beach.private_beach_id', true)
);

CREATE POLICY session_delete ON session
FOR DELETE
USING (
  private_beach_id::text = current_setting('beach.private_beach_id', true)
);

-- CONTROLLER EVENT (scoped via session)
DROP POLICY IF EXISTS controller_event_all ON controller_event;
CREATE POLICY controller_event_all ON controller_event
USING (
  EXISTS (
    SELECT 1 FROM session s
    WHERE s.id = controller_event.session_id
      AND s.private_beach_id::text = current_setting('beach.private_beach_id', true)
  )
);

-- CONTROLLER LEASE (scoped via session)
DROP POLICY IF EXISTS controller_lease_all ON controller_lease;
CREATE POLICY controller_lease_all ON controller_lease
USING (
  EXISTS (
    SELECT 1 FROM session s
    WHERE s.id = controller_lease.session_id
      AND s.private_beach_id::text = current_setting('beach.private_beach_id', true)
  )
)
WITH CHECK (
  EXISTS (
    SELECT 1 FROM session s
    WHERE s.id = controller_lease.session_id
      AND s.private_beach_id::text = current_setting('beach.private_beach_id', true)
  )
);

-- SESSION RUNTIME (scoped via session)
DROP POLICY IF EXISTS session_runtime_all ON session_runtime;
CREATE POLICY session_runtime_all ON session_runtime
USING (
  EXISTS (
    SELECT 1 FROM session s
    WHERE s.id = session_runtime.session_id
      AND s.private_beach_id::text = current_setting('beach.private_beach_id', true)
  )
)
WITH CHECK (
  EXISTS (
    SELECT 1 FROM session s
    WHERE s.id = session_runtime.session_id
      AND s.private_beach_id::text = current_setting('beach.private_beach_id', true)
  )
);

-- AUTOMATION ASSIGNMENT
DROP POLICY IF EXISTS automation_assignment_all ON automation_assignment;
CREATE POLICY automation_assignment_all ON automation_assignment
USING (
  private_beach_id::text = current_setting('beach.private_beach_id', true)
)
WITH CHECK (
  private_beach_id::text = current_setting('beach.private_beach_id', true)
);

-- FILE RECORD
DROP POLICY IF EXISTS file_record_all ON file_record;
CREATE POLICY file_record_all ON file_record
USING (
  private_beach_id::text = current_setting('beach.private_beach_id', true)
)
WITH CHECK (
  private_beach_id::text = current_setting('beach.private_beach_id', true)
);

-- PRIVATE BEACH
DROP POLICY IF EXISTS private_beach_all ON private_beach;
CREATE POLICY private_beach_all ON private_beach
USING (
  id::text = current_setting('beach.private_beach_id', true)
);

-- Note: We intentionally do NOT FORCE RLS here to avoid breaking existing owner connections.
-- CI/integration tests can enable FORCE RLS and run as a limited role to verify policies end-to-end.

