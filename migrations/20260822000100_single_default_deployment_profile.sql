-- A project (and the global fallback scope) may have at most one default profile.
-- Preserve the most recently changed profile when repairing legacy duplicate defaults.
WITH ranked_defaults AS (
    SELECT
        uuid,
        row_number() OVER (
            PARTITION BY (project_module IS NULL), COALESCE(project_module, '')
            ORDER BY updated_at DESC NULLS LAST, created_at DESC, uuid DESC
        ) AS rank
    FROM daliuge_deployment_profile
    WHERE is_default = true
)
UPDATE daliuge_deployment_profile AS profile
SET is_default = false,
    revision = revision + 1,
    spec_sha256 = NULL,
    updated_at = now()
FROM ranked_defaults
WHERE profile.uuid = ranked_defaults.uuid
  AND ranked_defaults.rank > 1;

CREATE UNIQUE INDEX IF NOT EXISTS uq_daliuge_profile_single_default
    ON daliuge_deployment_profile (
        (project_module IS NULL),
        (COALESCE(project_module, ''))
    )
    WHERE is_default = true;
