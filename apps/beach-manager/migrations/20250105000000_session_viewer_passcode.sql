ALTER TABLE session_runtime
    ADD COLUMN IF NOT EXISTS viewer_passcode TEXT;
