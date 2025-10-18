-- Optional audit column to record how a session mapping was established.
-- Non-breaking: defaults to NULL and is not enforced by RLS.
ALTER TABLE session
ADD COLUMN IF NOT EXISTS attach_method TEXT CHECK (attach_method IN ('code', 'owned', 'direct'));

