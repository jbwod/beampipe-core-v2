-- Jobs declare every capability needed to run them. A worker is eligible only
-- when its advertised capability set contains this complete requirement set.
ALTER TABLE jobs
    ADD COLUMN IF NOT EXISTS required_capabilities TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[];

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'jobs'
          AND column_name = 'required_capability'
    ) THEN
        UPDATE jobs
        SET required_capabilities = CASE
            WHEN kind IN ('scheduler_tick', 'discover_batch')
                THEN ARRAY['discovery:tap']::TEXT[]
            WHEN kind = 'execution_scheduler_tick'
                THEN ARRAY['manifest:generic']::TEXT[]
            WHEN kind IN ('dim_poll', 'dim_poll_tick')
                THEN ARRAY['deployment:daliuge_rest']::TEXT[]
            WHEN kind = 'slurm_poll_tick'
                THEN ARRAY['deployment:slurm_remote']::TEXT[]
            WHEN required_capability = 'discovery'
                THEN ARRAY['discovery:tap']::TEXT[]
            WHEN required_capability = 'manifest-generation'
                THEN ARRAY['manifest:generic']::TEXT[]
            WHEN required_capability = 'daliuge-deployment'
                THEN ARRAY['deployment:daliuge_rest']::TEXT[]
            WHEN required_capability = 'slurm-remote'
                THEN ARRAY['deployment:slurm_remote']::TEXT[]
            WHEN required_capability IS NULL
                THEN ARRAY[]::TEXT[]
            ELSE ARRAY[required_capability]
        END;

        -- Reconstruct the complete execute contract from immutable execution
        -- snapshots and the flags persisted in each job payload.
        UPDATE jobs AS j
        SET required_capabilities =
            CASE
                WHEN COALESCE(j.payload->'do_submit', 'true'::JSONB) = 'true'::JSONB
                THEN ARRAY[
                    CASE e.deployment_profile_snapshot #>> '{deployment,kind}'
                        WHEN 'rest_remote' THEN 'deployment:daliuge_rest'
                        WHEN 'slurm_remote' THEN 'deployment:slurm_remote'
                        ELSE 'routing:invalid_deployment_profile'
                    END
                ]::TEXT[]
                ELSE ARRAY[]::TEXT[]
            END
            || ARRAY['manifest:generic']::TEXT[]
            || CASE
                WHEN COALESCE(j.payload->'do_stage', 'true'::JSONB) <> 'true'::JSONB
                    THEN ARRAY[]::TEXT[]
                WHEN e.project_config_id IS NULL
                    THEN ARRAY['routing:invalid_project_config']::TEXT[]
                WHEN pc.spec #>> '{staging,provider}' = 'casda_uws'
                    THEN ARRAY['staging:casda_uws']::TEXT[]
                ELSE ARRAY[]::TEXT[]
            END
            || ARRAY['translation:daliuge']::TEXT[]
        FROM batch_execution_record AS e
        LEFT JOIN project_configs AS pc ON pc.uuid = e.project_config_id
        WHERE j.kind = 'execute'
          AND j.execution_id = e.uuid;

        UPDATE jobs
        SET required_capabilities = ARRAY[
            'manifest:generic',
            'routing:invalid_execution',
            'translation:daliuge'
        ]::TEXT[]
        WHERE kind = 'execute'
          AND execution_id IS NULL;

        ALTER TABLE jobs DROP COLUMN required_capability;
    END IF;
END $$;

ALTER TABLE jobs
    ADD CONSTRAINT ck_jobs_required_capabilities
    CHECK (
        array_position(required_capabilities, NULL) IS NULL
        AND NOT ('' = ANY(required_capabilities))
    );

CREATE INDEX IF NOT EXISTS idx_jobs_required_capabilities
    ON jobs USING GIN (required_capabilities);

CREATE INDEX IF NOT EXISTS idx_worker_instances_capabilities
    ON worker_instances USING GIN (capabilities);
