-- Capability and trust-boundary placement requirements for database-backed jobs.

ALTER TABLE jobs
    ADD COLUMN IF NOT EXISTS required_labels JSONB NOT NULL DEFAULT '{}'::JSONB;

CREATE INDEX IF NOT EXISTS idx_jobs_required_labels
    ON jobs USING GIN (required_labels jsonb_path_ops);

CREATE INDEX IF NOT EXISTS idx_jobs_active_pool
    ON jobs(pool, status, lease_expires_at)
    WHERE status = 'running';
