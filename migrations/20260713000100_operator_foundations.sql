-- Operator and distributed-execution foundations. All changes are additive except for
-- widening the existing job status check to include explicit operator review.

CREATE TABLE IF NOT EXISTS worker_instances (
    uuid UUID PRIMARY KEY,
    instance_name VARCHAR(128) NOT NULL,
    host_name VARCHAR(255) NOT NULL,
    process_id INTEGER,
    role VARCHAR(32) NOT NULL,
    pool VARCHAR(64) NOT NULL DEFAULT 'default',
    capabilities TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    labels JSONB NOT NULL DEFAULT '{}'::JSONB,
    version VARCHAR(64) NOT NULL,
    concurrency_limit INTEGER NOT NULL DEFAULT 1,
    status VARCHAR(32) NOT NULL DEFAULT 'active',
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    draining_at TIMESTAMPTZ,
    stopped_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT ck_worker_role CHECK (role IN ('worker', 'scheduler_worker')),
    CONSTRAINT ck_worker_status CHECK (status IN ('active', 'draining', 'stopped', 'unhealthy')),
    CONSTRAINT ck_worker_concurrency CHECK (concurrency_limit > 0)
);

CREATE INDEX IF NOT EXISTS idx_worker_instances_heartbeat
    ON worker_instances(status, last_heartbeat_at);
CREATE INDEX IF NOT EXISTS idx_worker_instances_pool
    ON worker_instances(pool, status);

ALTER TABLE jobs
    ADD COLUMN IF NOT EXISTS lease_owner UUID REFERENCES worker_instances(uuid) ON DELETE SET NULL,
    ADD COLUMN IF NOT EXISTS lease_token UUID,
    ADD COLUMN IF NOT EXISTS lease_expires_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS heartbeat_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS pool VARCHAR(64) NOT NULL DEFAULT 'default',
    ADD COLUMN IF NOT EXISTS required_capability VARCHAR(64),
    ADD COLUMN IF NOT EXISTS priority INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS failure_class VARCHAR(64),
    ADD COLUMN IF NOT EXISTS dead_lettered_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS dead_letter_reason TEXT;

ALTER TABLE jobs DROP CONSTRAINT IF EXISTS ck_job_status;
ALTER TABLE jobs
    ADD CONSTRAINT ck_job_status
    CHECK (status IN ('queued', 'running', 'completed', 'failed', 'dead_letter'));

UPDATE jobs
SET lease_expires_at = COALESCE(locked_until, now())
WHERE status = 'running' AND lease_expires_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_jobs_lease_recovery
    ON jobs(status, lease_expires_at)
    WHERE status = 'running';
CREATE INDEX IF NOT EXISTS idx_jobs_pool_claim
    ON jobs(pool, status, priority DESC, next_run_at, lease_expires_at);

CREATE TABLE IF NOT EXISTS job_claim_history (
    uuid UUID PRIMARY KEY,
    job_id UUID NOT NULL REFERENCES jobs(uuid) ON DELETE CASCADE,
    worker_id UUID REFERENCES worker_instances(uuid) ON DELETE SET NULL,
    lease_token UUID,
    event VARCHAR(32) NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    details JSONB NOT NULL DEFAULT '{}'::JSONB,
    CONSTRAINT ck_job_claim_event CHECK (
        event IN (
            'claimed', 'recovered', 'renewed', 'completed', 'requeued',
            'failed', 'dead_lettered', 'lease_lost', 'released'
        )
    )
);

CREATE INDEX IF NOT EXISTS idx_job_claim_history_job
    ON job_claim_history(job_id, occurred_at DESC);
CREATE INDEX IF NOT EXISTS idx_job_claim_history_worker
    ON job_claim_history(worker_id, occurred_at DESC)
    WHERE worker_id IS NOT NULL;

ALTER TABLE daliuge_deployment_profile
    ADD COLUMN IF NOT EXISTS revision INTEGER NOT NULL DEFAULT 1,
    ADD COLUMN IF NOT EXISTS spec_sha256 VARCHAR(64);

ALTER TABLE batch_execution_record
    ADD COLUMN IF NOT EXISTS deployment_profile_revision INTEGER,
    ADD COLUMN IF NOT EXISTS deployment_profile_snapshot JSONB,
    ADD COLUMN IF NOT EXISTS discovery_signature VARCHAR(64),
    ADD COLUMN IF NOT EXISTS manifest_sha256 VARCHAR(64),
    ADD COLUMN IF NOT EXISTS source_graph_sha256 VARCHAR(64),
    ADD COLUMN IF NOT EXISTS patched_graph_sha256 VARCHAR(64),
    ADD COLUMN IF NOT EXISTS physical_graph_sha256 VARCHAR(64),
    ADD COLUMN IF NOT EXISTS daliuge_session_id VARCHAR(255),
    ADD COLUMN IF NOT EXISTS daliuge_manager_url TEXT,
    ADD COLUMN IF NOT EXISTS remote_session_dir TEXT,
    ADD COLUMN IF NOT EXISTS control_phase VARCHAR(64),
    ADD COLUMN IF NOT EXISTS submission_state VARCHAR(32),
    ADD COLUMN IF NOT EXISTS scheduler_state VARCHAR(32),
    ADD COLUMN IF NOT EXISTS scheduler_raw_state VARCHAR(128),
    ADD COLUMN IF NOT EXISTS scheduler_reason TEXT,
    ADD COLUMN IF NOT EXISTS daliuge_state VARCHAR(32),
    ADD COLUMN IF NOT EXISTS daliuge_raw_status JSONB,
    ADD COLUMN IF NOT EXISTS output_state VARCHAR(32),
    ADD COLUMN IF NOT EXISTS output_verification_required BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS terminal_outcome VARCHAR(32),
    ADD COLUMN IF NOT EXISTS failure_class VARCHAR(64),
    ADD COLUMN IF NOT EXISTS phase_timestamps JSONB NOT NULL DEFAULT '{}'::JSONB,
    ADD COLUMN IF NOT EXISTS last_reconciled_at TIMESTAMPTZ;

UPDATE batch_execution_record
SET control_phase = COALESCE(control_phase, 'discovered'),
    submission_state = COALESCE(submission_state, 'not_started'),
    scheduler_state = COALESCE(scheduler_state, 'not_submitted'),
    daliuge_state = COALESCE(daliuge_state, 'not_created'),
    output_state = COALESCE(output_state, 'not_started');

ALTER TABLE batch_execution_record
    ALTER COLUMN control_phase SET DEFAULT 'discovered',
    ALTER COLUMN submission_state SET DEFAULT 'not_started',
    ALTER COLUMN scheduler_state SET DEFAULT 'not_submitted',
    ALTER COLUMN daliuge_state SET DEFAULT 'not_created',
    ALTER COLUMN output_state SET DEFAULT 'not_started';

-- Preserve the existing REST representation while giving new code a correctly named ID.
UPDATE batch_execution_record
SET daliuge_session_id = scheduler_job_id
WHERE scheduler_name = 'daliuge'
  AND scheduler_job_id IS NOT NULL
  AND daliuge_session_id IS NULL;

CREATE INDEX IF NOT EXISTS idx_execution_daliuge_session
    ON batch_execution_record(daliuge_session_id)
    WHERE daliuge_session_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_execution_scheduler_state
    ON batch_execution_record(scheduler_state, last_reconciled_at)
    WHERE scheduler_job_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_execution_daliuge_state
    ON batch_execution_record(daliuge_state, last_reconciled_at)
    WHERE daliuge_session_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS execution_observations (
    uuid UUID PRIMARY KEY,
    execution_id UUID NOT NULL REFERENCES batch_execution_record(uuid) ON DELETE CASCADE,
    kind VARCHAR(32) NOT NULL,
    normalized_state VARCHAR(32) NOT NULL,
    raw_state VARCHAR(128),
    reason TEXT,
    payload JSONB NOT NULL DEFAULT '{}'::JSONB,
    source_version VARCHAR(64),
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT ck_execution_observation_kind
        CHECK (kind IN ('scheduler', 'daliuge_session', 'daliuge_graph', 'output'))
);

CREATE INDEX IF NOT EXISTS idx_execution_observations_execution
    ON execution_observations(execution_id, observed_at DESC);

CREATE TABLE IF NOT EXISTS execution_artifacts (
    uuid UUID PRIMARY KEY,
    execution_id UUID NOT NULL REFERENCES batch_execution_record(uuid) ON DELETE CASCADE,
    kind VARCHAR(64) NOT NULL,
    storage_kind VARCHAR(32) NOT NULL,
    uri TEXT,
    inline_json JSONB,
    media_type VARCHAR(128) NOT NULL,
    sha256 VARCHAR(64) NOT NULL,
    size_bytes BIGINT,
    producer_phase VARCHAR(64) NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT ck_execution_artifact_storage
        CHECK (storage_kind IN ('database', 'file', 'remote', 'http')),
    CONSTRAINT ck_execution_artifact_location
        CHECK (uri IS NOT NULL OR inline_json IS NOT NULL),
    CONSTRAINT uq_execution_artifact UNIQUE(execution_id, kind, sha256)
);

CREATE INDEX IF NOT EXISTS idx_execution_artifacts_execution
    ON execution_artifacts(execution_id, created_at DESC);
