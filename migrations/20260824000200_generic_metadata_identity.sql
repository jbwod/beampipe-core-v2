DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'archive_metadata'
          AND column_name = 'sbid'
    ) AND NOT EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'archive_metadata'
          AND column_name = 'group_key'
    ) THEN
        ALTER TABLE archive_metadata RENAME COLUMN sbid TO group_key;
    END IF;
END $$;

ALTER TABLE archive_metadata
    ALTER COLUMN group_key TYPE VARCHAR(255);

ALTER TABLE archive_metadata
    DROP CONSTRAINT IF EXISTS uq_archive_metadata_composite;

ALTER TABLE archive_metadata
    ADD CONSTRAINT uq_archive_metadata_composite
    UNIQUE (project_module, source_identifier, group_key);

DROP INDEX IF EXISTS idx_archive_metadata_sbid;
CREATE INDEX IF NOT EXISTS idx_archive_metadata_group_key
    ON archive_metadata(group_key);
