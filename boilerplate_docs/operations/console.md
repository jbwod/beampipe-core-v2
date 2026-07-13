# Terminal console

`beampipe console` is a PostgreSQL-backed Ratatui interface for routine operations. It
uses the same repository and orchestration services as the CLI. Start it from an
environment with the same `DATABASE_URL` and layered configuration as the API workers.

```bash
beampipe console --refresh-ms 2000
```

## Views

| View | Data shown |
|------|------------|
| Overview | Sources, admissions, execution counts, queue depth, workers, alerts, integration health |
| Sources | Identifier, project, readiness, discovery signature, last discovery |
| Executions | Status, exact phase, scheduler, DALiuGE, and output axes |
| Workers | Instance, host, pool, heartbeat age, health, capabilities, active work |
| Scheduler | Profiles, jobs, normalized state, execution mapping, resource details |
| DALiuGE | Translator and manager health, version, nodes, sessions |
| Logs | Persisted structured provenance events and correlation identifiers |

## Keys

| Key | Action |
|-----|--------|
| `Tab`, `Shift-Tab`, arrows | Change view |
| `j`, `k`, arrows | Move selection |
| `/` | Filter the current data set |
| `Enter` | Inspect the selected row |
| `p` | Pause or resume refresh |
| `r` | Refresh now |
| `d` | Drain or resume a selected Beampipe control-plane worker |
| `c` | Cancel the selected active execution |
| `R` | Request a stage-aware retry for the selected failed execution |
| `?` | Context help |
| `q` | Quit |

Drain, cancellation, and retry require confirmation and are audited. Retry still passes
through the shared safety policy, so the console cannot override uncertain external
work. Terminals narrower than 70 columns receive a compact fallback view.

Every view remains available through CLI or `/api/v2`; the console is optional.
