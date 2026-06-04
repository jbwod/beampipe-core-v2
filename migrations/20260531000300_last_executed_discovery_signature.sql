ALTER TABLE source_registry
    ADD COLUMN IF NOT EXISTS last_executed_discovery_signature VARCHAR(64);
