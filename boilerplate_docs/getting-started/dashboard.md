# Dashboard tour

[Beampipe Dash](https://github.com/jbwod/beampipe-dash) is the optional web
operator console for Beampipe Core. It calls the authenticated `/api/v2`
interface through a server-side BFF; it does not own a database, execute jobs,
or infer workflow state independently of Core.

The screenshots on this page use Dash's checked-in synthetic Beampipe fixture.
They are safe documentation examples rather than evidence from a production
deployment.

## Control-plane overview

![Beampipe Dash operator overview showing dependency readiness, queue depth, workers, and recent outcomes](../assets/dash/operator-overview.jpg)

The overview brings several Core signals together for triage:

- authenticated readiness for PostgreSQL, queues, CASDA, and VizieR;
- registered sources and pending workflow admission;
- running and failed executions;
- active or stale workers and durable queue depth;
- recent API traffic and operator alerts.

Dash reports these values; Core's database, readiness probes, worker registry,
and execution ledger remain authoritative.

## Project studio

![Beampipe Dash project studio showing the WALLABY HiRes visual configuration beside canonical YAML](../assets/dash/project-studio.jpg)

The project studio keeps the form-based survey policy and canonical
`beampipe.dev/v2` YAML visible together. A successful save creates an immutable
project-config revision in Core. Executions pin a revision, so editing a later
version cannot silently change an existing run.

Use the Core [project YAML reference](../project-configs/index.md) for schema,
transform, query, graph, and automation semantics.

## Run detail

![Beampipe Dash run detail showing normalized execution state, phase timestamps, and pinned inputs](../assets/dash/run-detail.jpg)

The run explorer follows Core's durable execution model. It exposes normalized
control, submission, scheduler, DALiuGE, output, and terminal states alongside
phase timestamps and pinned inputs. Timeline, artifact, graph, manifest, and
ledger tabs retain the raw evidence needed for diagnosis.

Use the Core [execution state model](../architecture/state-machine.md) when
interpreting failed, uncertain, cancelled, or externally running work.

## Connect Dash to Core

For a single Docker engine, attach Dash to Core's private Compose network and
set `BEAMPIPE_API_URL=http://api:8080`. Publish Dash—not the Core API—to the
operator LAN or reverse proxy. See [Deployment topologies](deployment.md#core-and-dash-on-one-docker-engine)
for the complete Compose override and security boundary.

