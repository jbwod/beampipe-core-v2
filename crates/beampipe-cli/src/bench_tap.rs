use anyhow::Context;
use beampipe_adapters::{casda_tap, vizier_tap, TapClient};
use beampipe_domain::discovery::DiscoverySourceResult;
use beampipe_jobs::{ConfigDiscoveryRunner, DiscoveryRunner};
use beampipe_project::{build_template_context, ProjectConfig, TransformRegistry};
use std::path::Path;
use std::time::Instant;

#[derive(Debug, serde::Serialize)]
struct PhaseResult {
    phase: String,
    runs: u32,
    ok: u32,
    failed: u32,
    min_ms: u64,
    max_ms: u64,
    avg_ms: u64,
    p50_ms: u64,
    errors: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
struct BenchReport {
    source_identifier: String,
    casda_url: String,
    vizier_url: String,
    phases: Vec<PhaseResult>,
    full_discovery: PhaseResult,
    concurrent_full: Option<PhaseResult>,
}

pub async fn run(
    source: &str,
    config_path: &Path,
    runs: u32,
    concurrent: Option<usize>,
) -> anyhow::Result<()> {
    let bytes = std::fs::read(config_path)
        .with_context(|| format!("read config {}", config_path.display()))?;
    let config = ProjectConfig::from_slice(&bytes)?;
    let casda_url = std::env::var("BEAMPIPE_CASDA_TAP_URL")
        .unwrap_or_else(|_| "https://casda.csiro.au/casda_vo_tools/tap/sync".into());
    let vizier_url = std::env::var("BEAMPIPE_VIZIER_TAP_URL")
        .unwrap_or_else(|_| "https://tapvizier.cds.unistra.fr/TAPVizieR/tap".into());

    let casda = casda_tap(&casda_url);
    let vizier = vizier_tap(&vizier_url);

    let registry = TransformRegistry::from_config(&config);
    let context = build_template_context(source, &config);
    let source_name = context
        .get("source_name")
        .and_then(|v| v.as_str())
        .unwrap_or(source);
    let visibility_adql = render_wallaby_visibility(source);
    let vizier_adql = format!(
        r#"SELECT HIPASS, RAJ2000, DEJ2000, RV50max, RV50min, RVmom FROM "VIII/73/hicat" WHERE HIPASS = '{source_name}'"#
    );

    let mut phases = Vec::new();
    phases.push(
        bench_phase("casda_visibility", runs, |_i| {
            let client = casda.clone();
            let adql = visibility_adql.clone();
            async move {
                let rows = client.query_rows(&adql).await?;
                Ok(format!("rows={}", rows.len()))
            }
        })
        .await,
    );
    phases.push(
        bench_phase("vizier_ra_dec_vsys", runs, |_i| {
            let client = vizier.clone();
            let adql = vizier_adql.clone();
            async move {
                let rows = client.query_rows(&adql).await?;
                Ok(format!("rows={}", rows.len()))
            }
        })
        .await,
    );

    // Resolve first SBID from visibility for eval-file phase
    let eval_phase = match casda.query_rows(&visibility_adql).await {
        Ok(rows) => {
            let sbid = rows
                .first()
                .and_then(|r| r.get("obs_id"))
                .and_then(|v| registry.apply_named("extract_askap_sbid", v))
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            if sbid.is_empty() {
                PhaseResult {
                    phase: "casda_eval_file".into(),
                    runs: 0,
                    ok: 0,
                    failed: 1,
                    min_ms: 0,
                    max_ms: 0,
                    avg_ms: 0,
                    p50_ms: 0,
                    errors: vec!["no sbid from visibility query".into()],
                }
            } else {
                let eval_adql = format!(
                    "SELECT * FROM casda.observation_evaluation_file WHERE sbid = '{sbid}'"
                );
                bench_phase("casda_eval_file", runs, {
                    let client = casda.clone();
                    let adql = eval_adql.clone();
                    let sbid_label = sbid.clone();
                    move |_i| {
                        let client = client.clone();
                        let adql = adql.clone();
                        let sbid_label = sbid_label.clone();
                        async move {
                            let rows = client.query_rows(&adql).await?;
                            Ok(format!("sbid={sbid_label} rows={}", rows.len()))
                        }
                    }
                })
                .await
            }
        }
        Err(e) => PhaseResult {
            phase: "casda_eval_file".into(),
            runs: 0,
            ok: 0,
            failed: 1,
            min_ms: 0,
            max_ms: 0,
            avg_ms: 0,
            p50_ms: 0,
            errors: vec![e.to_string()],
        },
    };
    phases.push(eval_phase);

    let runner = ConfigDiscoveryRunner::from_env();
    let full = bench_phase("full_discover_source", runs, {
        let cfg = config.clone();
        move |_i| {
            let runner = runner.clone();
            let cfg = cfg.clone();
            let source = source.to_string();
            async move {
                let result = runner
                    .discover_source(Some(&cfg), &cfg.metadata.id, &source)
                    .await;
                match result {
                    DiscoverySourceResult::HasMetadata { metadata, .. } => {
                        Ok(format!("datasets={}", metadata.len()))
                    }
                    DiscoverySourceResult::NoDatasets { .. } => Ok("no_datasets".into()),
                    DiscoverySourceResult::Unchanged { .. } => Ok("unchanged".into()),
                    DiscoverySourceResult::Error { error, .. } => Err(anyhow::anyhow!(error)),
                    DiscoverySourceResult::Timeout { error, .. } => {
                        Err(anyhow::anyhow!("timeout: {error}"))
                    }
                }
            }
        }
    })
    .await;

    let concurrent_full = if let Some(n) = concurrent.filter(|&n| n > 1) {
        Some(bench_concurrent_full(&config, source, runs.min(n as u32), n).await)
    } else {
        None
    };

    let report = BenchReport {
        source_identifier: source.to_string(),
        casda_url,
        vizier_url,
        phases,
        full_discovery: full,
        concurrent_full,
    };

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

async fn bench_concurrent_full(
    config: &ProjectConfig,
    source: &str,
    runs: u32,
    concurrency: usize,
) -> PhaseResult {
    let mut latencies = Vec::new();
    let mut errors = Vec::new();
    let mut ok = 0u32;
    let mut failed = 0u32;

    for round in 0..runs {
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
        let mut handles = Vec::new();
        for _ in 0..concurrency {
            let permit = match sem.clone().acquire_owned().await {
                Ok(p) => p,
                Err(_) => break,
            };
            let runner = ConfigDiscoveryRunner::from_env();
            let cfg = config.clone();
            let source = source.to_string();
            handles.push(tokio::spawn(async move {
                let _permit = permit;
                let start = Instant::now();
                let result = runner
                    .discover_source(Some(&cfg), &cfg.metadata.id, &source)
                    .await;
                (start.elapsed(), result)
            }));
        }
        for handle in handles {
            match handle.await {
                Ok((elapsed, result)) => match result {
                    DiscoverySourceResult::HasMetadata { .. }
                    | DiscoverySourceResult::NoDatasets { .. }
                    | DiscoverySourceResult::Unchanged { .. } => {
                        ok += 1;
                        latencies.push(elapsed.as_millis() as u64);
                    }
                    DiscoverySourceResult::Error { error, .. } => {
                        failed += 1;
                        errors.push(error);
                    }
                    DiscoverySourceResult::Timeout { error, .. } => {
                        failed += 1;
                        errors.push(format!("timeout: {error}"));
                    }
                },
                Err(e) => {
                    failed += 1;
                    errors.push(format!("round {round} join: {e}"));
                }
            }
        }
    }

    summarize(
        &format!("full_discover_x{concurrency}"),
        runs * concurrency as u32,
        ok,
        failed,
        latencies,
        errors,
    )
}

async fn bench_phase<F, Fut>(name: &str, runs: u32, mut f: F) -> PhaseResult
where
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<String>>,
{
    let mut latencies = Vec::with_capacity(runs as usize);
    let mut errors = Vec::new();
    let mut ok = 0u32;
    let mut failed = 0u32;

    for i in 0..runs {
        let start = Instant::now();
        match f(i).await {
            Ok(detail) => {
                ok += 1;
                latencies.push(start.elapsed().as_millis() as u64);
                eprintln!(
                    "[bench] {name} run {} ok ({detail}) {}ms",
                    i + 1,
                    latencies.last().unwrap()
                );
            }
            Err(e) => {
                failed += 1;
                errors.push(e.to_string());
                eprintln!("[bench] {name} run {} failed: {e}", i + 1);
            }
        }
    }

    summarize(name, runs, ok, failed, latencies, errors)
}

fn summarize(
    name: &str,
    runs: u32,
    ok: u32,
    failed: u32,
    mut latencies: Vec<u64>,
    errors: Vec<String>,
) -> PhaseResult {
    if latencies.is_empty() {
        return PhaseResult {
            phase: name.into(),
            runs,
            ok,
            failed,
            min_ms: 0,
            max_ms: 0,
            avg_ms: 0,
            p50_ms: 0,
            errors,
        };
    }
    latencies.sort_unstable();
    let sum: u64 = latencies.iter().sum();
    PhaseResult {
        phase: name.into(),
        runs,
        ok,
        failed,
        min_ms: latencies[0],
        max_ms: *latencies.last().unwrap(),
        avg_ms: sum / latencies.len() as u64,
        p50_ms: latencies[latencies.len() / 2],
        errors,
    }
}

fn render_wallaby_visibility(source: &str) -> String {
    format!(
        r#"SELECT o.* FROM ivoa.obscore o
INNER JOIN (
  SELECT MAX(t_max) AS mx FROM ivoa.obscore
  WHERE filename LIKE '{source}%'
  AND obs_collection IN ('ASKAP Pilot Survey for WALLABY', 'WALLABY')
) AS latest ON o.t_max = latest.mx
WHERE o.filename LIKE '{source}%'
AND o.obs_collection IN ('ASKAP Pilot Survey for WALLABY', 'WALLABY')"#
    )
}
