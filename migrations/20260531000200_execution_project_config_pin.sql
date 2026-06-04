ALTER TABLE batch_execution_record
    ADD COLUMN IF NOT EXISTS project_config_id UUID REFERENCES project_configs(uuid);

CREATE INDEX IF NOT EXISTS idx_batch_execution_record_project_config_id
    ON batch_execution_record(project_config_id);
