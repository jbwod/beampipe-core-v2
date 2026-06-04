-- Remove ephemeral rows left by `cargo test` against a shared DATABASE_URL.
-- Safe to run in dev; does not touch real project modules like wallaby_hires.

DELETE FROM source_registry
WHERE project_module LIKE 'fail_requeue_%'
   OR project_module LIKE 'sig_test_%'
   OR project_module LIKE 'test_%'
   OR project_module LIKE 'exec_sig_%';
