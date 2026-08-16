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

<section class="bp-install-builder" id="install-builder" data-bp-install-builder>
  <form class="bp-install-builder__form" action="#" method="get">
    <fieldset>
      <legend>install.sh</legend>
      <div class="bp-install-builder__grid">
        <div class="bp-install-builder__field">
          <label for="bp-install-runtime">Runtime</label>
          <select id="bp-install-runtime" name="runtime">
            <option value="docker" selected>Docker Compose</option>
            <option value="host">Host binary</option>
          </select>
        </div>
        <div class="bp-install-builder__field">
          <label for="bp-install-directory">Directory</label>
          <input id="bp-install-directory" name="directory" type="text" spellcheck="false" placeholder="~/beampipe" autocomplete="off">
        </div>
        <div class="bp-install-builder__checks">
          <label>
            <input id="bp-install-yes" name="yes" type="checkbox" checked>
            Non-interactive (<code>--yes</code>)
          </label>
          <label>
            <input id="bp-install-start" name="start" type="checkbox" checked>
            Start Postgres and the stack
          </label>
          <label>
            <input id="bp-install-dashboard" name="dashboard" type="checkbox">
            Prepare Dash (Docker only)
          </label>
        </div>
        <div class="bp-install-builder__field">
          <label for="bp-install-admin-user">Admin username</label>
          <input id="bp-install-admin-user" name="admin-user" type="text" spellcheck="false" placeholder="admin" autocomplete="username">
        </div>
        <div class="bp-install-builder__field">
          <label for="bp-install-admin-email">Admin email</label>
          <input id="bp-install-admin-email" name="admin-email" type="email" spellcheck="false" placeholder="admin@example.test" autocomplete="email">
        </div>
        <div class="bp-install-builder__field">
          <label for="bp-install-admin-password">Admin password</label>
          <input id="bp-install-admin-password" name="admin-password" type="password" placeholder="generated if empty" autocomplete="new-password">
        </div>
      </div>
      <p class="bp-install-builder__note">Leave the password empty to generate one at setup. A typed password is included in the command and will sit in shell history.</p>
    </fieldset>
  </form>
  <div class="bp-install-builder__output">
    <div class="bp-install-builder__command-row">
      <pre><code id="bp-install-command" aria-live="polite">curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh | sh -s -- --yes --runtime docker</code></pre>
      <button type="button" class="terminal-button" id="bp-install-copy">Copy</button>
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
