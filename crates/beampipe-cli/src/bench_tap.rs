use anyhow::Context;
use beampipe_domain::discovery::DiscoverySourceResult;
use beampipe_jobs::{ConfigDiscoveryRunner, DiscoveryRunner};
use beampipe_project::ProjectConfig;
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
    project_id: String,
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
                        Ok(format!("records={}", metadata.len()))
                    }
                    DiscoverySourceResult::NoRecords { .. } => Ok("no_records".into()),
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
        project_id: config.metadata.id.clone(),
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
                    | DiscoverySourceResult::NoRecords { .. }
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
