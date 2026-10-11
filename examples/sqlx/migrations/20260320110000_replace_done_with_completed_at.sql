-- Replace the done flag with the time the todo was completed.
ALTER TABLE todos ADD COLUMN completed_at TIMESTAMPTZ;
UPDATE todos SET completed_at = created_at WHERE done;
ALTER TABLE todos DROP COLUMN done;
