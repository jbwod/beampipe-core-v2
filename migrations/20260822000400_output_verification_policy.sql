-- Pin the output-verification contract used when each execution was admitted.
ALTER TABLE batch_execution_record
    ADD COLUMN IF NOT EXISTS output_verification_policy JSONB NOT NULL
    DEFAULT '{"required":false,"inventory_schema":"wallaby-hires-output-inventory/v1"}'::JSONB;

UPDATE batch_execution_record
SET output_verification_policy = jsonb_build_object(
        'required', output_verification_required,
        'inventory_schema', 'wallaby-hires-output-inventory/v1'
    );
