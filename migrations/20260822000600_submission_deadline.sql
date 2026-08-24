-- Bound the period during which an accepted submission intent may still be
-- executing in a worker. The deadline is persisted per attempt so recovery
-- decisions do not depend on later configuration changes.
ALTER TABLE batch_execution_record
    ADD COLUMN IF NOT EXISTS submission_deadline_at TIMESTAMPTZ;
