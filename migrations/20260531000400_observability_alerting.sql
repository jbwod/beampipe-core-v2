-- Provenance event stream
CREATE TABLE IF NOT EXISTS provenance_events (
    id UUID PRIMARY KEY,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    event_type TEXT NOT NULL,
    project_module TEXT NOT NULL,
    source_identifier TEXT,
    execution_id UUID,
    actor TEXT,
    correlation_id TEXT,
    payload JSONB NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX IF NOT EXISTS idx_provenance_events_project_occurred
    ON provenance_events (project_module, occurred_at DESC);
CREATE INDEX IF NOT EXISTS idx_provenance_events_execution
    ON provenance_events (execution_id, occurred_at DESC)
    WHERE execution_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_provenance_events_source
    ON provenance_events (project_module, source_identifier, occurred_at DESC)
    WHERE source_identifier IS NOT NULL;

-- Notification channels and alert rules
CREATE TABLE IF NOT EXISTS notification_channels (
    uuid UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL CHECK (kind IN ('webhook', 'email')),
    config JSONB NOT NULL DEFAULT '{}'::jsonb,
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS alert_rules (
    uuid UUID PRIMARY KEY,
    name TEXT NOT NULL,
    project_module TEXT,
    enabled BOOLEAN NOT NULL DEFAULT true,
    severity TEXT NOT NULL DEFAULT 'warning',
    trigger_kind TEXT NOT NULL,
    trigger_config JSONB NOT NULL DEFAULT '{}'::jsonb,
    channel_ids UUID[] NOT NULL DEFAULT '{}',
    cooldown_minutes INT NOT NULL DEFAULT 60,
    last_fired_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_alert_rules_project ON alert_rules (project_module);

CREATE TABLE IF NOT EXISTS alert_deliveries (
    uuid UUID PRIMARY KEY,
    rule_id UUID REFERENCES alert_rules (uuid) ON DELETE SET NULL,
    channel_id UUID REFERENCES notification_channels (uuid) ON DELETE SET NULL,
    status TEXT NOT NULL,
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_alert_deliveries_created ON alert_deliveries (created_at DESC);
