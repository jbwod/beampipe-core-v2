# Discovery and execution lifecycle

The lifecycle starts with an operator registering a source and ends with an execution record containing manifest, backend status, and provenance.

## Discovery

<div class="terminal-diagram">
<pre>register source
      |
      v
mark for discovery
      |
      v
scheduler_tick
      |
      v
discover_batch
      |
      v
TAP queries -> field map -> metadata rows
                         |
                         v
                discovery signature
                         |
                         v
                workflow_run_pending</pre>
</div>

Important controls:

| Control | Source |
|---------|--------|
| Query templates | `project_config.discovery.queries` |
| Enrichment queries | `project_config.discovery.enrichments` |
| Field mapping | `project_config.discovery.prepare_metadata.field_map` |
| Signature fields | `project_config.discovery.prepare_metadata.signature` |
| Batch limits | `automation.discovery` and `BEAMPIPE_SHAPING_*` |

## Execution

<div class="terminal-diagram">
<pre>pending source(s)
      |
      v
create execution
      |
      v
build manifest -> graph patches -> stage / translate / submit
      |                |                         |
      v                v                         v
ledger row       prepared graph             backend run
      |                                          |
      v                                          v
poll tick <---------- REST DIM or Slurm status --+
      |
      v
completed / failed / cancelled</pre>
</div>

Important controls:

| Control | Source |
|---------|--------|
| Grouping | `manifest.group_by` |
| Manifest shape | `manifest.source_template`, `dataset_template`, `path` |
| Graph mutation | `graph_patches` |
| Backend selection | `deployment_profile_name` or project default |
| Admission | `automation.execution` and execution shaping env vars |

## Provenance

Each state transition can emit provenance rows. Use execution, source, and project event endpoints to explain why a run is pending, blocked, failed, or complete.
