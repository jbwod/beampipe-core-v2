---
hide:
  - toc
---

# Start with the path that matches the work

<p class="bp-lede">Beampipe can be explored locally without touching scientific infrastructure, connected to an existing DALiuGE service, or prepared for scheduler-backed production. Choose the boundary you need to cross today.</p>

<div class="bp-switcher bp-terminal-frame" data-bp-switcher data-title="start.path">
  <div class="bp-segmented" role="tablist" aria-label="Getting-started path">
    <button type="button" role="tab" aria-selected="true" aria-controls="path-local" id="tab-local" data-bp-target="path-local">local evaluation</button>
    <button type="button" role="tab" aria-selected="false" aria-controls="path-service" id="tab-service" data-bp-target="path-service">existing DALiuGE</button>
    <button type="button" role="tab" aria-selected="false" aria-controls="path-hpc" id="tab-hpc" data-bp-target="path-hpc">HPC / Setonix</button>
  </div>

  <section id="path-local" role="tabpanel" aria-labelledby="tab-local" data-bp-panel>
    <span class="bp-status" data-tone="green">SAFE / MOCK</span>
    <h2>Prove the control plane locally</h2>
    <p>Start PostgreSQL, install the WALLABY configuration, run diagnostics, and open the real operator console. Archive, scheduler, and DALiuGE calls stay disabled.</p>
    <p><a class="terminal-button" href="five-minute-start/">Run the five-minute start</a></p>
  </section>

  <section id="path-service" role="tabpanel" aria-labelledby="tab-service" data-bp-panel hidden>
    <span class="bp-status" data-tone="cyan">REST / REMOTE</span>
    <h2>Connect to translator and manager services</h2>
    <p>Install the binary, run the setup wizard with a <code>rest_remote</code> profile, verify TLS and credentials, then prepare a graph without submitting it.</p>
    <p><a class="terminal-button" href="../operations/daliuge-setonix/">Configure DALiuGE REST</a></p>
  </section>

  <section id="path-hpc" role="tabpanel" aria-labelledby="tab-hpc" data-bp-panel hidden>
    <span class="bp-status" data-tone="amber">SLURM / LIVE</span>
    <h2>Prepare a production scheduler path</h2>
    <p>Model the facility profile, establish SSH trust, validate <code>squeue</code>, <code>sacct</code>, and <code>sbatch</code>, then rehearse graph preparation before enabling real backends.</p>
    <p><a class="terminal-button" href="../operations/daliuge-setonix/#trust-and-credentials">Prepare the HPC boundary</a></p>
  </section>
</div>

## The safe progression

<div class="bp-flow-diagram" role="img" aria-label="Safe progression from local setup to live work">
  <div class="bp-flow-node" data-tone="cyan"><span>01</span><strong>install</strong><small>binary + PostgreSQL</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>02</span><strong>mock</strong><small>local profile</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>03</span><strong>preflight</strong><small>real endpoints</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>04</span><strong>dry run</strong><small>manifest + graph</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>05</span><strong>submit</strong><small>observed live work</small></div>
</div>

Do not skip from installation to live submission. Beampipe makes external uncertainty explicit; the setup path should do the same.

## Local command deck

<div class="terminal-command" data-title="bootstrap.local">

```bash
docker compose up -d postgres
mkdir -p operator-local
beampipe init --directory operator-local
cd operator-local
beampipe setup --yes \
  --admin-password 'replace-this-local-password' \
  --project-config ../config/wallaby_hires.v2.yaml
beampipe doctor
beampipe start
```

</div>

The setup command writes private local configuration, applies migrations, creates the administrator, installs a deployment profile, validates the project, and runs diagnostics. It refuses to replace a different profile silently.

## What to read next

| You need to | Continue with |
|---|---|
| Understand prerequisites and release artifacts | [Installation](installation.md) |
| Complete one source-to-execution workflow | [First workflow](first-run.md) |
| Explain an environment variable or precedence rule | [Application configuration](configuration.md) |
| Start an operator shift | [Operator handbook](../operations/index.md) |
| Understand why Beampipe owns state but not science execution | [Architecture map](../architecture/index.md) |
