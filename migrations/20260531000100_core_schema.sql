CREATE TABLE IF NOT EXISTS users (
    id SERIAL PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE,
    name VARCHAR(30) NOT NULL,
    username VARCHAR(20) NOT NULL UNIQUE,
    email VARCHAR(50) NOT NULL UNIQUE,
    hashed_password TEXT NOT NULL,
    profile_image_url TEXT NOT NULL DEFAULT 'https://profileimageurl.com',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ,
    deleted_at TIMESTAMPTZ,
    is_deleted BOOLEAN NOT NULL DEFAULT false,
    is_superuser BOOLEAN NOT NULL DEFAULT false
);

CREATE INDEX IF NOT EXISTS idx_users_username ON users(username);
CREATE INDEX IF NOT EXISTS idx_users_email ON users(email);
CREATE INDEX IF NOT EXISTS idx_users_is_deleted ON users(is_deleted);

CREATE TABLE IF NOT EXISTS token_blacklist (
    id SERIAL PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS daliuge_deployment_profile (
    uuid UUID PRIMARY KEY,
    name VARCHAR(50) NOT NULL UNIQUE,
    description VARCHAR(255),
    project_module VARCHAR(50),
    is_default BOOLEAN NOT NULL DEFAULT false,
    translation JSONB NOT NULL DEFAULT '{}'::jsonb,
    deployment JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_daliuge_profile_project_default
    ON daliuge_deployment_profile(project_module, is_default);

CREATE TABLE IF NOT EXISTS source_registry (
    uuid UUID PRIMARY KEY,
    project_module VARCHAR(50) NOT NULL,
    source_identifier VARCHAR(100) NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_checked_at TIMESTAMPTZ,
    last_attempted_at TIMESTAMPTZ,
    stale_after_hours INTEGER,
    discovery_signature VARCHAR(64),
    discovery_claim_token VARCHAR(64),
    discovery_claim_expires_at TIMESTAMPTZ,
    workflow_run_pending BOOLEAN NOT NULL DEFAULT false,
    workflow_run_pending_at TIMESTAMPTZ,
    workflow_claim_token VARCHAR(64),
    workflow_claimed_at TIMESTAMPTZ,
    workflow_claim_expires_at TIMESTAMPTZ,
    CONSTRAINT uq_source_registry_composite UNIQUE(project_module, source_identifier)
);

CREATE INDEX IF NOT EXISTS idx_source_registry_project ON source_registry(project_module);
CREATE INDEX IF NOT EXISTS idx_source_registry_identifier ON source_registry(source_identifier);
CREATE INDEX IF NOT EXISTS idx_source_registry_enabled ON source_registry(enabled);
CREATE INDEX IF NOT EXISTS idx_source_registry_discovery_claim_expires_at
    ON source_registry(discovery_claim_expires_at);
CREATE INDEX IF NOT EXISTS idx_source_registry_project_pending
    ON source_registry(project_module, workflow_run_pending);

CREATE TABLE IF NOT EXISTS archive_metadata (
    uuid UUID PRIMARY KEY,
    project_module VARCHAR(50) NOT NULL,
    source_identifier VARCHAR(100) NOT NULL,
    sbid VARCHAR(50) NOT NULL,
    metadata_json JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ,
    CONSTRAINT uq_archive_metadata_composite UNIQUE(project_module, source_identifier, sbid)
);

CREATE INDEX IF NOT EXISTS idx_archive_metadata_project_source
    ON archive_metadata(project_module, source_identifier);
CREATE INDEX IF NOT EXISTS idx_archive_metadata_sbid ON archive_metadata(sbid);

CREATE TABLE IF NOT EXISTS batch_execution_record (
    uuid UUID PRIMARY KEY,
    project_module VARCHAR(50) NOT NULL,
    sources JSONB NOT NULL,
    archive_name VARCHAR(50) NOT NULL,
    deployment_profile_id UUID REFERENCES daliuge_deployment_profile(uuid),
    workflow_manifest JSONB,
    execution_phase VARCHAR(32),
    scheduler_name VARCHAR(50),
    scheduler_job_id VARCHAR(512),
    last_error TEXT,
    created_by_id INTEGER REFERENCES users(id),
    status VARCHAR(32) NOT NULL DEFAULT 'pending',
    retry_count INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    CONSTRAINT ck_execution_status CHECK (
        status IN (
            'pending', 'running', 'awaiting_scheduler', 'not_submitted',
            'completed', 'failed', 'retrying', 'cancelled'
        )
    ),
    CONSTRAINT ck_execution_phase CHECK (
        execution_phase IS NULL OR execution_phase IN ('stage_and_manifest', 'submit')
    )
);

CREATE INDEX IF NOT EXISTS idx_batch_execution_record_project ON batch_execution_record(project_module);
CREATE INDEX IF NOT EXISTS idx_batch_execution_record_status ON batch_execution_record(status);
CREATE INDEX IF NOT EXISTS idx_batch_execution_record_scheduler_job_id
    ON batch_execution_record(scheduler_job_id);
CREATE INDEX IF NOT EXISTS idx_batch_execution_record_deployment_profile_id
    ON batch_execution_record(deployment_profile_id);

CREATE TABLE IF NOT EXISTS project_configs (
    uuid UUID PRIMARY KEY,
    project_id VARCHAR(80) NOT NULL,
    version INTEGER NOT NULL,
    spec JSONB NOT NULL,
    spec_sha256 VARCHAR(64) NOT NULL,
    active BOOLEAN NOT NULL DEFAULT true,
    uploaded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT uq_project_config_version UNIQUE(project_id, version)
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_project_config_active
    ON project_configs(project_id)
    WHERE active;
CREATE INDEX IF NOT EXISTS idx_project_configs_project ON project_configs(project_id);

CREATE TABLE IF NOT EXISTS project_config_wasm (
    uuid UUID PRIMARY KEY,
    project_config_id UUID NOT NULL REFERENCES project_configs(uuid) ON DELETE CASCADE,
    wasm_sha256 VARCHAR(64) NOT NULL,
    wasm_bytes BYTEA NOT NULL,
    uploaded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT uq_project_config_wasm_sha UNIQUE(project_config_id, wasm_sha256)
);

CREATE TABLE IF NOT EXISTS jobs (
    uuid UUID PRIMARY KEY,
    kind VARCHAR(64) NOT NULL,
    payload JSONB NOT NULL,
    status VARCHAR(32) NOT NULL DEFAULT 'queued',
    execution_id UUID REFERENCES batch_execution_record(uuid),
    phase VARCHAR(64),
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 3,
    next_run_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    locked_until TIMESTAMPTZ,
    idempotency_key TEXT,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ,
    CONSTRAINT ck_job_status CHECK (status IN ('queued', 'running', 'completed', 'failed'))
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_jobs_idempotency_key
    ON jobs(idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_jobs_claim
    ON jobs(status, next_run_at, locked_until);
CREATE INDEX IF NOT EXISTS idx_jobs_execution_id ON jobs(execution_id);
CREATE INDEX IF NOT EXISTS idx_jobs_kind ON jobs(kind);

