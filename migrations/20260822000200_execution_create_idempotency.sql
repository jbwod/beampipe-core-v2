ALTER TABLE batch_execution_record
    ADD COLUMN IF NOT EXISTS create_idempotency_key TEXT,
    ADD COLUMN IF NOT EXISTS create_request_sha256 TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS uq_execution_create_idempotency
    ON batch_execution_record(created_by_id, create_idempotency_key)
    WHERE created_by_id IS NOT NULL AND create_idempotency_key IS NOT NULL;
