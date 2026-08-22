-- Cancellation of an execution must also fence its queued/running execute jobs.
ALTER TABLE jobs DROP CONSTRAINT IF EXISTS ck_job_status;
ALTER TABLE jobs
    ADD CONSTRAINT ck_job_status
    CHECK (status IN ('queued', 'running', 'completed', 'failed', 'dead_letter', 'cancelled'));
