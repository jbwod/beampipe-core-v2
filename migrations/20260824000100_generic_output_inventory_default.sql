-- Make output verification a Beampipe contract rather than a project contract.
-- Existing execution policies used the former Wallaby schema name; migrate those
-- rows so in-flight and historical executions continue to describe the single
-- schema accepted by Core.
ALTER TABLE batch_execution_record
    ALTER COLUMN output_verification_policy
    SET DEFAULT '{"required":false,"inventory_schema":"beampipe-output-inventory/v1"}'::JSONB;

UPDATE batch_execution_record
SET output_verification_policy = jsonb_set(
        output_verification_policy,
        '{inventory_schema}',
        '"beampipe-output-inventory/v1"'::JSONB
    )
WHERE output_verification_policy ->> 'inventory_schema'
    = 'wallaby-hires-output-inventory/v1';
