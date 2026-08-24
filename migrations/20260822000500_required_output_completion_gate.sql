-- Repair aggregate success rows created before backend polling respected the
-- required-output gate. Preserve scheduler/DALiuGE evidence, but reopen the
-- aggregate execution until a trusted publication inventory is committed.
WITH repaired AS (
    UPDATE batch_execution_record
    SET status = CASE
            WHEN daliuge_state = 'finished'
             AND scheduler_state IN ('succeeded', 'not_submitted')
            THEN 'running'
            ELSE 'failed'
        END,
        execution_phase = NULL,
        control_phase = CASE
            WHEN daliuge_state = 'finished'
             AND scheduler_state IN ('succeeded', 'not_submitted')
            THEN 'output_verification'
            ELSE 'terminal'
        END,
        terminal_outcome = CASE
            WHEN daliuge_state = 'finished'
             AND scheduler_state IN ('succeeded', 'not_submitted')
            THEN NULL
            ELSE 'inconsistent'
        END,
        failure_class = CASE
            WHEN daliuge_state = 'finished'
             AND scheduler_state IN ('succeeded', 'not_submitted')
            THEN NULL
            ELSE 'inconsistent_state'
        END,
        last_error = CASE
            WHEN daliuge_state = 'finished'
             AND scheduler_state IN ('succeeded', 'not_submitted')
            THEN NULL
            ELSE 'legacy aggregate completion contradicted external execution axes'
        END,
        workflow_manifest = CASE
            WHEN daliuge_state = 'finished'
             AND scheduler_state IN ('succeeded', 'not_submitted')
            THEN (workflow_manifest #- '{beampipe_run_record,slurm,terminal,ledger_status}')
                 #- '{beampipe_run_record,dim,terminal,ledger_status}'
            ELSE workflow_manifest
        END,
        phase_timestamps = CASE
            WHEN daliuge_state = 'finished'
             AND scheduler_state IN ('succeeded', 'not_submitted')
            THEN (phase_timestamps - 'terminal')
                 || jsonb_build_object('output_verification', to_jsonb(now()))
            WHEN phase_timestamps ? 'terminal' THEN phase_timestamps
            ELSE phase_timestamps || jsonb_build_object('terminal', to_jsonb(now()))
        END,
        completed_at = CASE
            WHEN daliuge_state = 'finished'
             AND scheduler_state IN ('succeeded', 'not_submitted')
            THEN NULL
            ELSE COALESCE(completed_at, now())
        END,
        last_reconciled_at = now(),
        updated_at = now()
    WHERE output_verification_required = true
      AND status = 'completed'
      AND COALESCE(output_state, '') <> 'verified'
    RETURNING uuid, project_module, sources, scheduler_state, daliuge_state, status
)
INSERT INTO provenance_events (
    id, event_type, project_module, source_identifier,
    execution_id, actor, correlation_id, payload
)
SELECT gen_random_uuid(),
       'execution.reconciliation_mismatch',
       project_module,
       sources->0->>'source_identifier',
       uuid,
       'system:migration',
       uuid::text,
       jsonb_build_object(
           'code', 'required_output_completion_reopened',
           'message', 'legacy completion without verified outputs was repaired',
           'scheduler_state', scheduler_state,
           'daliuge_state', daliuge_state,
           'repaired_status', status,
           'requires_operator', status = 'failed'
       )
FROM repaired;

ALTER TABLE batch_execution_record
    ADD CONSTRAINT ck_required_output_completion_verified
    CHECK (
        status <> 'completed'
        OR output_verification_required = false
        OR COALESCE(output_state, '') = 'verified'
    );
