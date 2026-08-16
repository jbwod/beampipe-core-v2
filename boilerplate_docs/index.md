---
hide:
  - toc
---

<div class="bp-docs-hero" markdown>

<p class="bp-hero-kicker">[ RUST V2 / DURABLE CONTROL PLANE ]</p>

# beampipe

<p class="bp-tagline">Discover archive data, prepare reproducible DALiuGE graphs, and operate REST or Slurm execution from one PostgreSQL-backed control plane.</p>

<div class="bp-hero-actions" markdown>
[Start locally](getting-started/index.md){ .terminal-button }
[Configure a backend](architecture/deployment-profiles.md){ .terminal-button }
</div>

<div class="bp-hero-status" aria-label="Current product boundary">
  <span><b data-tone="cyan">API</b> /api/v2</span>
  <span><b data-tone="amber">TRUTH</b> PostgreSQL</span>
  <span><b data-tone="green">RUNTIME</b> REST or Slurm</span>
</div>

</div>

## Fastest safe start

Run the API, durable job system, and console locally. External execution stays mocked until you explicitly enable real backends. Discovery uses live TAP only after you register and trigger a source. Build the installer command, then follow the [quick start](getting-started/index.md).

<section class="bp-install-builder" id="install-builder" data-bp-install-builder aria-labelledby="bp-install-title">
  <header class="bp-install-builder__chrome">
    <p class="bp-install-builder__title" id="bp-install-title">[ install.sh ]</p>
    <p class="bp-install-builder__meta"><b data-tone="cyan">CMD</b> curl | sh</p>
    <p class="bp-install-builder__meta"><b data-tone="green">LIVE</b> builder</p>
  </header>
  <form class="bp-install-builder__form" action="#" method="get">
    <div class="bp-install-builder__body">
      <div class="bp-install-builder__section">
        <p class="bp-install-builder__kicker">01 / runtime</p>
        <div class="bp-install-builder__chips" role="radiogroup" aria-label="Runtime">
          <label class="bp-install-builder__chip">
            <input id="bp-install-runtime-docker" name="runtime" type="radio" value="docker" checked>
            <span>Docker Compose</span>
          </label>
          <label class="bp-install-builder__chip">
            <input id="bp-install-runtime-host" name="runtime" type="radio" value="host">
            <span>Host binary</span>
          </label>
        </div>
        <div class="bp-install-builder__field">
          <label for="bp-install-directory">Directory</label>
          <input id="bp-install-directory" name="directory" type="text" spellcheck="false" placeholder="~/beampipe" autocomplete="off">
        </div>
      </div>
      <div class="bp-install-builder__section">
        <p class="bp-install-builder__kicker">02 / flags</p>
        <div class="bp-install-builder__checks">
          <label class="bp-install-builder__toggle">
            <input id="bp-install-yes" name="yes" type="checkbox" checked>
            <span>Non-interactive <code>--yes</code></span>
          </label>
          <label class="bp-install-builder__toggle">
            <input id="bp-install-start" name="start" type="checkbox" checked>
            <span>Start Postgres and the stack</span>
          </label>
          <label class="bp-install-builder__toggle" id="bp-install-dashboard-label">
            <input id="bp-install-dashboard" name="dashboard" type="checkbox">
            <span>Prepare Dash <small>Docker only</small></span>
          </label>
        </div>
      </div>
      <div class="bp-install-builder__section">
        <p class="bp-install-builder__kicker">03 / admin</p>
        <div class="bp-install-builder__admin">
          <div class="bp-install-builder__field">
            <label for="bp-install-admin-user">Username</label>
            <input id="bp-install-admin-user" name="admin-user" type="text" spellcheck="false" placeholder="admin" autocomplete="username">
          </div>
          <div class="bp-install-builder__field">
            <label for="bp-install-admin-email">Email</label>
            <input id="bp-install-admin-email" name="admin-email" type="email" spellcheck="false" placeholder="admin@example.test" autocomplete="email">
          </div>
          <div class="bp-install-builder__field">
            <label for="bp-install-admin-password">Password</label>
            <input id="bp-install-admin-password" name="admin-password" type="password" placeholder="generated if empty" autocomplete="new-password">
          </div>
        </div>
        <p class="bp-install-builder__note">Empty password generates one at setup. A typed password is quoted into the command and will sit in shell history.</p>
      </div>
    </div>
  </form>
  <div class="bp-install-builder__output">
    <div class="bp-install-builder__prompt">
      <span class="bp-install-builder__ps" aria-hidden="true">$</span>
      <pre><code id="bp-install-command" aria-live="polite">curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh | sh -s -- --yes --runtime docker</code></pre>
      <button type="button" class="terminal-button bp-install-builder__copy" id="bp-install-copy">Copy</button>
    </div>
    <p class="bp-install-builder__hint">API at <code>http://127.0.0.1:8080/api/v2</code>. Files in <code>~/beampipe</code> unless you set a directory. Dash stays opt-in.</p>
    <p class="bp-install-builder__status" id="bp-install-status" aria-live="polite"></p>
  </div>
  <noscript>
    <pre><code>curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh | sh
curl -fsS http://127.0.0.1:8080/api/v2/health</code></pre>
  </noscript>
</section>

## How work moves

<div class="bp-flow-diagram bp-flow-diagram--wide bp-flow-diagram--animated" role="img" aria-label="Archive discovery and project policy flow through the Beampipe ledger and workers to DALiuGE">
  <div class="bp-flow-node" data-tone="cyan"><span>01 / FACTS</span><strong>CASDA + VizieR</strong><small>project-defined TAP</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>02 / INTENT</span><strong>PostgreSQL</strong><small>config + ledger + jobs</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>03 / EFFECT</span><strong>workers</strong><small>leased and fenced</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>04 / RUNTIME</span><strong>DALiuGE</strong><small>REST DIM or Slurm</small></div>
</div>

Beampipe owns durable intent, preparation artifacts, admission, submission records, and reconciliation. Archives, Slurm, and DALiuGE remain authoritative for their own facts. A successful scheduler submission is therefore never treated as proof of a successful graph.

## Choose a task

<div class="bp-feature-grid bp-feature-grid--routes">
<a href="getting-started/"><strong>[01] Set up</strong><span>Reach a healthy local control plane, then run one source workflow.</span></a>
<a href="operations/"><strong>[02] Operate</strong><span>Watch queues and workers, investigate failures, and recover safely.</span></a>
<a href="project-configs/"><strong>[03] Configure</strong><span>Define TAP queries, metadata, manifests, graph patches, and automation.</span></a>
<a href="architecture/deployment-profiles/"><strong>[04] Deploy</strong><span>Connect an existing DALiuGE DIM or a Slurm facility with strict SSH trust.</span></a>
<a href="architecture/"><strong>[05] Understand</strong><span>Follow durable state through discovery, preparation, submission, and polling.</span></a>
<a href="api/"><strong>[06] Integrate</strong><span>Use the authenticated API workflow and generated schema.</span></a>
</div>

## Current qualification

The implementation has been exercised through real CASDA/VizieR discovery, automatic admission, manifest and graph preparation, DALiuGE translation, and REST deployment to a local cluster. That run exposed and fixed manifest flag resolution. A graph/runtime package mismatch then prevented terminal graph success, and CASDA staging plus Slurm have not yet been qualified end to end.

Use the [qualification run](operations/end-to-end-demo.md) for the exact evidence required before calling a release production-ready.
