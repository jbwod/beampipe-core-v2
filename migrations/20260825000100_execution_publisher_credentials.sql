CREATE TABLE IF NOT EXISTS execution_publisher_credentials (
    uuid UUID PRIMARY KEY,
    execution_id UUID NOT NULL
        REFERENCES batch_execution_record(uuid) ON DELETE CASCADE,
    token_hash VARCHAR(64) NOT NULL UNIQUE,
    audience TEXT NOT NULL,
    scope TEXT NOT NULL,
    execution_attempt INTEGER NOT NULL,
    issued_by UUID REFERENCES users(uuid),
    issued_by_actor TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    request_sha256 VARCHAR(64),
    output_artifact_id UUID REFERENCES execution_artifacts(uuid),
    revoked_at TIMESTAMPTZ,
    revoked_reason TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT execution_publisher_credentials_scope_exact CHECK (
        scope = 'execution:' || execution_id::text || ':verify_outputs'
    ),
    CONSTRAINT execution_publisher_credentials_audience_exact CHECK (
        audience = 'beampipe-output-verification'
    ),
    CONSTRAINT execution_publisher_credentials_attempt_nonnegative CHECK (
        execution_attempt >= 0
    ),
    CONSTRAINT execution_publisher_credentials_consumption_complete CHECK (
        (consumed_at IS NULL AND request_sha256 IS NULL AND output_artifact_id IS NULL)
        OR
        (consumed_at IS NOT NULL AND request_sha256 IS NOT NULL AND output_artifact_id IS NOT NULL)
    ),
    CONSTRAINT execution_publisher_credentials_request_sha256 CHECK (
        request_sha256 IS NULL OR request_sha256 ~ '^[0-9a-f]{64}$'
    ),
    CONSTRAINT execution_publisher_credentials_issuer CHECK (
        length(issued_by_actor) BETWEEN 1 AND 256
    ),
    CONSTRAINT execution_publisher_credentials_revocation_complete CHECK (
        (revoked_at IS NULL AND revoked_reason IS NULL)
        OR
        (revoked_at IS NOT NULL AND length(revoked_reason) BETWEEN 1 AND 256)
    )
);

CREATE INDEX IF NOT EXISTS idx_execution_publisher_credentials_execution
    ON execution_publisher_credentials(execution_id, expires_at DESC);

ALTER TABLE batch_execution_record
    ALTER COLUMN output_verification_policy
    SET DEFAULT '{"required":false,"inventory_schema":"beampipe-output-inventory/v1","expected_patterns":[]}'::JSONB;

UPDATE batch_execution_record
SET output_verification_policy = output_verification_policy
    || jsonb_build_object('expected_patterns', '[]'::JSONB)
WHERE NOT (output_verification_policy ? 'expected_patterns');
