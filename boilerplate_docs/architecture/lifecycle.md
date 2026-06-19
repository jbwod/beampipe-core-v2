# Discovery and execution lifecycle

The lifecycle starts with an operator registering a source and ends with an execution record containing manifest, backend status, and provenance.

![Terminal-style Beampipe execution lifecycle diagram](../assets/readme/execution-lifecycle-terminal-dark.png)

## Discovery

| Control | Source |
|---------|--------|
| Query templates | `project_config.discovery.queries` |
| Enrichment queries | `project_config.discovery.enrichments` |
| Field mapping | `project_config.discovery.prepare_metadata.field_map` |
| Signature fields | `project_config.discovery.prepare_metadata.signature` |
| Batch limits | `automation.discovery` and `BEAMPIPE_SHAPING_*` |

Discovery signatures determine whether prepared metadata changed enough to trigger future work. Exclude volatile archive fields such as access URLs, file sizes, and timestamps when they should not cause reruns.

## Execution

| Control | Source |
|---------|--------|
| Grouping | `manifest.group_by` |
| Manifest shape | `manifest.source_template`, `dataset_template`, `path` |
| Graph mutation | `graph_patches` YAML, documented as DALiuGE Graphs |
| Backend selection | `deployment_profile_name` or project default |
| Admission | `automation.execution` and execution shaping variables |

## Operator model

Beampipe separates authoritative state, backend detail, and audit narrative:

| Layer | Storage | Use when |
|-------|---------|----------|
| Execution ledger | `batch_execution_record` | FSM truth, list/filter runs, cancel |
| Run record | `workflow_manifest.beampipe_run_record` | Backend integration detail, poll counters, raw excerpts |
| Provenance | `provenance_events` | Operator timeline: discovery changes, execution transitions, alerts |
| Metrics | `beampipe_*` on `:9090` | Dashboards and alert thresholds |

Recommended debug order for a stuck run: readiness, metrics, provenance events, then `beampipe_run_record` in the execution response.

Next: choose backend behavior in [Deployment profiles](deployment-profiles.md).
