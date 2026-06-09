-- Provenance retention: purge events older than BEAMPIPE_PROVENANCE_RETENTION_DAYS (default 90).
-- Run via: beampipe purge-provenance  (or cron).

CREATE INDEX IF NOT EXISTS idx_provenance_events_occurred_at
    ON provenance_events (occurred_at);

COMMENT ON TABLE provenance_events IS 'Append-only audit stream; purge old rows with beampipe purge-provenance';
