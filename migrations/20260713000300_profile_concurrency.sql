-- Deployment-profile admission limit. NULL means no profile-specific cap.

ALTER TABLE daliuge_deployment_profile
    ADD COLUMN IF NOT EXISTS max_concurrent_executions INTEGER;

ALTER TABLE daliuge_deployment_profile
    DROP CONSTRAINT IF EXISTS ck_deployment_profile_concurrency;
ALTER TABLE daliuge_deployment_profile
    ADD CONSTRAINT ck_deployment_profile_concurrency
    CHECK (max_concurrent_executions IS NULL OR max_concurrent_executions > 0);

CREATE INDEX IF NOT EXISTS idx_execution_active_profile
    ON batch_execution_record(deployment_profile_id, status)
    WHERE status IN ('pending', 'running', 'awaiting_scheduler', 'retrying');
