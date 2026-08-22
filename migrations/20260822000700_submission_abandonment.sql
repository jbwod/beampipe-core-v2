-- Operator abandonment is a durable safety barrier for unresolved external
-- submissions. It never asserts that the remote job does not exist.
ALTER TABLE batch_execution_record
    ADD COLUMN IF NOT EXISTS submission_abandoned_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_execution_submission_abandoned
    ON batch_execution_record(submission_abandoned_at)
    WHERE submission_abandoned_at IS NOT NULL
      AND scheduler_name = 'slurm'
      AND scheduler_job_id IS NULL;
