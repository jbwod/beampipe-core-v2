# Dashboard boundary and architecture

Beampipe Dash is an authenticated operator client, not a second control plane.
It owns no database, scheduler, worker, credential store, or workflow state.
Every displayed value and mutation maps to Core `/api/v2`; PostgreSQL remains
the durable ledger.

<div class="bp-boundary-map" role="img" aria-label="The operator browser connects to the Dashboard Next.js application and same-origin proxy. Dashboard connects privately to the Core API. Core owns PostgreSQL, the scheduler, workers, and external archive and execution integrations.">
  <section data-tone="cyan"><span>OPERATOR DEVICE</span><strong>browser UI</strong><small>same-origin requests only</small></section>
  <span class="bp-boundary-map__link" aria-hidden="true">--&gt;</span>
  <section data-tone="green"><span>BEAMPIPE DASH</span><strong>Next.js + BFF</strong><small>HttpOnly cookie boundary<br><code>/api/beampipe</code></small></section>
  <span class="bp-boundary-map__link" aria-hidden="true">--&gt;</span>
  <section data-tone="amber"><span>CORE CONTROL PLANE</span><strong>Rust <code>/api/v2</code></strong><small>PostgreSQL + scheduler + workers</small></section>
  <span class="bp-boundary-map__link" aria-hidden="true">--&gt;</span>
  <section data-tone="cyan"><span>AUTHORITIES</span><strong>TAP + DALiuGE + Slurm</strong><small>archive and runtime facts</small></section>
</div>

Client JavaScript never reads a Core token: the browser stores access and
refresh tokens only in HttpOnly cookies set by Dash. Dash forwards authenticated
intent; Core and its PostgreSQL ledger remain authoritative.

## Authentication boundary

<ol class="bp-sequence-diagram" aria-label="Dashboard authentication and token refresh sequence">
  <li><span>01</span><strong>Browser &rarr; Dash</strong><code>POST /api/session/login</code><small>credentials stay on the same origin</small></li>
  <li><span>02</span><strong>Dash &rarr; Core</strong><code>POST /api/v2/login</code><small>server-to-server authentication</small></li>
  <li><span>03</span><strong>Core &rarr; Dash</strong><code>access + refresh</code><small>Dash writes HttpOnly, SameSite=Lax cookies; Secure is configuration-dependent</small></li>
  <li><span>04</span><strong>Browser &rarr; Dash &rarr; Core</strong><code>GET /api/beampipe/executions</code><small>the BFF adds the bearer access token</small></li>
  <li><span>05 / IF EXPIRED</span><strong>Dash &harr; Core</strong><code>POST /api/v2/refresh</code><small>rotate once, then replay the original request</small></li>
  <li><span>06</span><strong>Core &rarr; Dash &rarr; Browser</strong><code>redacted response</code><small>tokens never enter client JavaScript</small></li>
</ol>

The generic proxy accepts only `/api/v2/*`, discards caller-supplied
`Authorization` headers, rejects cross-origin mutations, and never exposes
either token to client JavaScript. Refresh requests are coalesced within one
Dash process. Until refresh coordination uses a shared store, run one Dash
replica or configure sticky affinity so one browser session stays on one
replica.

## Data ownership

| Data | Authority | Dashboard responsibility |
|---|---|---|
| Users and password hashes | Core | Login and current-user lookup only |
| Project configurations | Core immutable revisions | Visual/YAML authoring and upload |
| Deployment profiles | Core revisioned rows | Typed REST/Slurm editing and connectivity checks |
| Sources and archive metadata | Core | Registry, discovery trigger, and readiness inspection |
| Jobs and worker leases | Core | Bounded polling and privileged scheduler actions |
| Executions and artifacts | Core ledger | Prepare, create, start, retry, cancel, and inspect |
| SSH keys and external secrets | Core runtime | Readiness metadata only; never accepted or stored |

## Project studio contract

YAML is canonical. The visual editor and code surface operate on one parsed
`ProjectConfig` draft:

1. A visual edit updates the in-memory draft and serialized YAML.
2. Valid YAML immediately normalizes the visual draft.
3. Invalid YAML remains visible while the last valid visual draft is preserved.
4. Unknown project-specific keys survive normalization and round trips.
5. Saving uploads a new immutable revision and displays Core diagnostics.

TAP queries remain project-defined. Dash does not hardcode CASDA or VizieR
ADQL, duplicate source readiness rules, or infer backend transitions.

## Runtime behavior

- Live views poll at bounded 5–30 second intervals.
- Terminal execution detail stops automatic polling; use manual refresh for a
  deliberate re-read.
- Mutations invalidate related views rather than clearing all cached state.
- Execution preparation is authoritative and any changed selection invalidates
  the previous preview.
- Deployment forms mirror Core's tagged REST/Slurm profile schema.
- Destructive or externally effective operations require confirmation or an
  explicit review step.
- An ambiguous run-creation response is frozen with the same idempotency key;
  retrying never silently creates a second operator intent.

## Interface map

| Route | Purpose |
|---|---|
| `/overview` | Dependency state, API traffic, queue, workers, latest runs, launchpad |
| `/projects` and `/projects/new` | Project registry and bidirectional visual/YAML studio |
| `/profiles` | REST/Slurm profiles, revisions, resources, and connectivity |
| `/sources` and `/sources/:id` | Registration, discovery, admission, metadata, provenance |
| `/runs/new` | Multi-source preparation, profile pinning, creation, submission |
| `/runs` and `/runs/:id` | Live/history list and execution evidence explorer |
| `/jobs` | Durable queue, attempts, leases, and errors |
| `/workers` | Worker pools, capabilities, health, and active leases |
| `/alerts` | Core notification channels, rules, tests, and redacted deliveries |
| `/system` | Readiness, diagnostics, reconciliation risk, and SSH posture |

User creation remains a Core CLI bootstrap operation because `/api/v2` does
not expose user administration. Dash deliberately does not create a competing
credential store.
