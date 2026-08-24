# Quick start

The recommended installation needs Docker with Compose v2 and ports `5432`, `18080`, and `9090`. Override host ports with `--api-port`, `--postgres-port`, and `--metrics-port`.

```bash
curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh | sh
```

Choose Docker and the managed PostgreSQL service. Setup writes `~/beampipe`, creates private random credentials, and starts API, scheduler, and worker services. It does not select a scientific project for you.

Verify without changing directories:

```bash
beampipe status
beampipe doctor
curl -fsS http://127.0.0.1:18080/api/v2/health
```

Common operations:

```bash
beampipe logs --follow
beampipe restart
beampipe stop
beampipe start
beampipe uninstall
```

Add your project configuration explicitly:

```bash
beampipe project add -f PROJECT_CONFIG
```

To follow the WALLABY HiRes walkthrough, opt into its sample bundle during installation instead:

```bash
beampipe setup --yes --runtime docker --sample wallaby-hires
```

External execution remains disabled until a typed deployment profile is installed and checked. With `--yes`, do those steps afterwards:

```bash
beampipe profile add -f PROFILE_CONFIG
beampipe doctor --profile PROFILE_NAME
# then set BEAMPIPE_USE_REAL_BACKENDS=true in ~/beampipe/.env and run beampipe restart
```

Continue with:

1. [Install and configure](installation.md) for Docker, native host, and source-build paths.
2. [Deployment profiles and SSH](../architecture/deployment-profiles.md) for REST/DIM or Slurm.
3. [First workflow](first-run.md) to register and discover a source.
4. [Local DALiuGE end to end](local-daliuge.md) to qualify real discovery,
   translation, REST deployment, reconciliation, and artifacts with the
   no-download graph.
5. [Dashboard setup and tour](dashboard.md) for the optional web console.
