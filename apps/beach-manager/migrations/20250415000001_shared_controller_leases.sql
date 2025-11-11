-- Allow multiple active controller leases per session by dropping the unique
-- constraint on session_id and adding a partial index that keeps lookups fast.
ALTER TABLE public.controller_lease
    DROP CONSTRAINT IF EXISTS controller_lease_session_id_key;

CREATE INDEX IF NOT EXISTS idx_controller_lease_session_expires
    ON public.controller_lease (session_id, expires_at)
    WHERE revoked_at IS NULL;
