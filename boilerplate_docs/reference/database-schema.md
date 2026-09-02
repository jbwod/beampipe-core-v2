# Database schema explorer

Explore the PostgreSQL control-plane schema produced by every checked-in SQLx
migration. Search by table, column, or PostgreSQL type; narrow the inventory by
domain; then follow declared foreign keys through the relationship map.

!!! info "Catalog snapshot, not a live database console"

    The explorer reads a committed catalog snapshot generated from a disposable
    PostgreSQL 16 database. It contains structure only—no operator data,
    credentials, row counts, or query access. Runtime truth remains the database
    reached by `DATABASE_URL`.

<div
  class="bp-db-explorer"
  data-bp-database-explorer
  data-schema-url="../../assets/data/database-schema.json"
  aria-busy="true"
>
  <div class="bp-db-explorer__chrome" aria-label="Database schema summary">
    <span><b>DATABASE</b> public</span>
    <span data-bp-schema-summary>loading catalog…</span>
    <span data-bp-schema-migration>migration —</span>
  </div>

  <div class="bp-db-explorer__toolbar">
    <label class="bp-db-explorer__search">
      <span>find</span>
      <input
        type="search"
        autocomplete="off"
        placeholder="table, column, or type"
        aria-label="Search database tables, columns, and types"
        data-bp-schema-search
      >
    </label>
    <div class="bp-db-explorer__filters" aria-label="Filter tables by domain">
      <button type="button" aria-pressed="true" data-bp-schema-group="all">all</button>
      <button type="button" aria-pressed="false" data-bp-schema-group="ledger">ledger</button>
      <button type="button" aria-pressed="false" data-bp-schema-group="work">work</button>
      <button type="button" aria-pressed="false" data-bp-schema-group="discovery">discovery</button>
      <button type="button" aria-pressed="false" data-bp-schema-group="config">config</button>
      <button type="button" aria-pressed="false" data-bp-schema-group="identity">identity</button>
      <button type="button" aria-pressed="false" data-bp-schema-group="alerts">alerts</button>
    </div>
  </div>

  <div class="bp-db-explorer__metrics" aria-label="Schema totals">
    <span><b data-bp-schema-metric="tables">—</b> tables</span>
    <span><b data-bp-schema-metric="columns">—</b> columns</span>
    <span><b data-bp-schema-metric="relationships">—</b> foreign keys</span>
    <span><b data-bp-schema-metric="indexes">—</b> indexes</span>
  </div>

  <div class="bp-db-explorer__body">
    <aside class="bp-db-explorer__inventory" aria-label="Database tables">
      <div class="bp-db-explorer__inventory-head">
        <span>tables</span>
        <span data-bp-schema-result-count>—</span>
      </div>
      <div data-bp-schema-table-list role="listbox" aria-label="Database tables">
        <p class="bp-db-explorer__loading">Loading schema snapshot…</p>
      </div>
    </aside>

    <section class="bp-db-explorer__detail" data-bp-schema-detail aria-live="polite">
      <p class="bp-db-explorer__loading">Preparing table detail…</p>
    </section>
  </div>

  <section class="bp-db-explorer__map" aria-labelledby="bp-db-map-title">
    <div class="bp-db-explorer__map-head">
      <div>
        <span>relationship map</span>
        <strong id="bp-db-map-title">Declared foreign keys</strong>
      </div>
      <small>Select a node to inspect its columns and dependencies.</small>
    </div>
    <div data-bp-schema-map></div>
  </section>

  <noscript>
    <p class="bp-db-explorer__error">
      JavaScript is required for filtering and relationship navigation. The raw
      <a href="../../assets/data/database-schema.json">catalog snapshot</a> remains available.
    </p>
  </noscript>
</div>

## Persistence domains

| Domain | Tables | Contract |
| --- | --- | --- |
| Execution ledger | `batch_execution_record`, `execution_observations`, `execution_artifacts`, `provenance_events` | Durable intent, external evidence, immutable artifacts, and audit history |
| Distributed work | `jobs`, `job_claim_history`, `worker_instances`, `daliuge_deployment_profile` | Claims, leases, fencing, placement, and deployment policy |
| Discovery | `source_registry`, `archive_metadata` | Source enrollment, claims, signatures, and normalized archive facts |
| Project configuration | `project_configs`, `project_config_wasm` | Versioned project specifications and content-addressed extensions |
| Identity | `users`, `token_blacklist` | Operator identities and revoked access tokens |
| Alerting | `alert_rules`, `notification_channels`, `alert_deliveries` | Alert policy, delivery targets, and delivery outcomes |

The relationship map shows PostgreSQL foreign-key constraints only.
`provenance_events.execution_id` is deliberately an indexed audit correlation
field rather than a foreign key, so provenance can outlive or describe records
without introducing a deletion dependency.

## Refresh the snapshot

Whenever a migration changes schema, regenerate the explorer data before
committing:

```bash
make db-schema
git diff -- boilerplate_docs/assets/data/database-schema.json
```

The exporter starts a disposable `postgres:16-alpine` container, applies the
migrations in filename order, reads `pg_catalog`, validates the JSON, and removes
the container. Set `BEAMPIPE_DOCKER_CONTEXT` when the desired Docker engine is
not the current context.
