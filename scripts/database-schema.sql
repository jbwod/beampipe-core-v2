WITH foreign_keys AS (
    SELECT
        constraint_row.conname AS name,
        source_table.relname AS source_table,
        source_column.attname AS source_column,
        target_table.relname AS target_table,
        target_column.attname AS target_column,
        CASE constraint_row.confdeltype
            WHEN 'a' THEN 'NO ACTION'
            WHEN 'r' THEN 'RESTRICT'
            WHEN 'c' THEN 'CASCADE'
            WHEN 'n' THEN 'SET NULL'
            WHEN 'd' THEN 'SET DEFAULT'
        END AS on_delete
    FROM pg_constraint AS constraint_row
    JOIN pg_class AS source_table
      ON source_table.oid = constraint_row.conrelid
    JOIN pg_namespace AS source_namespace
      ON source_namespace.oid = source_table.relnamespace
    JOIN pg_class AS target_table
      ON target_table.oid = constraint_row.confrelid
    JOIN generate_subscripts(constraint_row.conkey, 1) AS key_position
      ON true
    JOIN pg_attribute AS source_column
      ON source_column.attrelid = source_table.oid
     AND source_column.attnum = constraint_row.conkey[key_position]
    JOIN pg_attribute AS target_column
      ON target_column.attrelid = target_table.oid
     AND target_column.attnum = constraint_row.confkey[key_position]
    WHERE source_namespace.nspname = 'public'
      AND constraint_row.contype = 'f'
),
schema_tables AS (
    SELECT
        table_row.oid,
        table_row.relname AS name,
        obj_description(table_row.oid) AS description
    FROM pg_class AS table_row
    JOIN pg_namespace AS table_namespace
      ON table_namespace.oid = table_row.relnamespace
    WHERE table_namespace.nspname = 'public'
      AND table_row.relkind = 'r'
),
schema_columns AS (
    SELECT
        table_row.name AS table_name,
        column_row.attnum AS position,
        column_row.attname AS name,
        format_type(column_row.atttypid, column_row.atttypmod) AS type,
        NOT column_row.attnotnull AS nullable,
        pg_get_expr(default_row.adbin, default_row.adrelid) AS default_value,
        EXISTS (
            SELECT 1
            FROM pg_constraint AS primary_key
            WHERE primary_key.conrelid = table_row.oid
              AND primary_key.contype = 'p'
              AND column_row.attnum = ANY(primary_key.conkey)
        ) AS primary_key,
        (
            SELECT jsonb_build_object(
                'table', foreign_key.target_table,
                'column', foreign_key.target_column,
                'on_delete', foreign_key.on_delete
            )
            FROM foreign_keys AS foreign_key
            WHERE foreign_key.source_table = table_row.name
              AND foreign_key.source_column = column_row.attname
            LIMIT 1
        ) AS references
    FROM schema_tables AS table_row
    JOIN pg_attribute AS column_row
      ON column_row.attrelid = table_row.oid
     AND column_row.attnum > 0
     AND NOT column_row.attisdropped
    LEFT JOIN pg_attrdef AS default_row
      ON default_row.adrelid = table_row.oid
     AND default_row.adnum = column_row.attnum
),
schema_indexes AS (
    SELECT
        table_row.name AS table_name,
        index_table.relname AS name,
        index_row.indisunique AS unique,
        access_method.amname AS method,
        pg_get_indexdef(index_table.oid) AS definition
    FROM schema_tables AS table_row
    JOIN pg_index AS index_row
      ON index_row.indrelid = table_row.oid
    JOIN pg_class AS index_table
      ON index_table.oid = index_row.indexrelid
    JOIN pg_am AS access_method
      ON access_method.oid = index_table.relam
    WHERE NOT index_row.indisprimary
),
schema_checks AS (
    SELECT
        table_row.name AS table_name,
        constraint_row.conname AS name,
        pg_get_constraintdef(constraint_row.oid, true) AS definition
    FROM schema_tables AS table_row
    JOIN pg_constraint AS constraint_row
      ON constraint_row.conrelid = table_row.oid
    WHERE constraint_row.contype = 'c'
)
SELECT jsonb_pretty(
    jsonb_build_object(
        'schema', 'public',
        'generated_from', 'migrations',
        'migration_count', :migration_count,
        'latest_migration', :'latest_migration',
        'table_count', (SELECT count(*) FROM schema_tables),
        'column_count', (SELECT count(*) FROM schema_columns),
        'index_count', (SELECT count(*) FROM schema_indexes),
        'relationship_count', (SELECT count(*) FROM foreign_keys),
        'tables', (
            SELECT jsonb_agg(
                jsonb_build_object(
                    'name', table_row.name,
                    'description', table_row.description,
                    'columns', (
                        SELECT jsonb_agg(
                            jsonb_build_object(
                                'name', column_row.name,
                                'type', column_row.type,
                                'nullable', column_row.nullable,
                                'default', column_row.default_value,
                                'primary_key', column_row.primary_key,
                                'references', column_row.references
                            )
                            ORDER BY column_row.position
                        )
                        FROM schema_columns AS column_row
                        WHERE column_row.table_name = table_row.name
                    ),
                    'indexes', (
                        SELECT COALESCE(
                            jsonb_agg(
                                jsonb_build_object(
                                    'name', index_row.name,
                                    'unique', index_row.unique,
                                    'method', index_row.method,
                                    'definition', index_row.definition
                                )
                                ORDER BY index_row.name
                            ),
                            '[]'::jsonb
                        )
                        FROM schema_indexes AS index_row
                        WHERE index_row.table_name = table_row.name
                    ),
                    'checks', (
                        SELECT COALESCE(
                            jsonb_agg(
                                jsonb_build_object(
                                    'name', check_row.name,
                                    'definition', check_row.definition
                                )
                                ORDER BY check_row.name
                            ),
                            '[]'::jsonb
                        )
                        FROM schema_checks AS check_row
                        WHERE check_row.table_name = table_row.name
                    )
                )
                ORDER BY table_row.name
            )
            FROM schema_tables AS table_row
        ),
        'relationships', (
            SELECT jsonb_agg(
                jsonb_build_object(
                    'name', foreign_key.name,
                    'from_table', foreign_key.source_table,
                    'from_column', foreign_key.source_column,
                    'to_table', foreign_key.target_table,
                    'to_column', foreign_key.target_column,
                    'on_delete', foreign_key.on_delete
                )
                ORDER BY foreign_key.source_table, foreign_key.source_column
            )
            FROM foreign_keys AS foreign_key
        )
    )
);
