use crate::{HttpTapAdapter, TapClient, TapMode};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct TapEndpointProbe {
    pub url: String,
    pub mode: TapMode,
}

#[derive(Debug, Default)]
pub struct TapHealthCache {
    state: Mutex<BTreeMap<String, TapHealthCacheSlot>>,
}

const MAX_TAP_HEALTH_CACHE_ENTRIES: usize = 256;

#[derive(Debug, Clone)]
struct CachedTapHealth {
    checked_at: Instant,
    report: TapHealthReport,
}

#[derive(Debug)]
struct TapHealthCacheSlot {
    last_accessed: Instant,
    entry: Arc<TapHealthCacheEntry>,
}

#[derive(Debug, Default)]
struct TapHealthCacheEntry {
    cached: Mutex<Option<CachedTapHealth>>,
}

impl TapHealthCache {
    /// Return a revision-keyed cached report, or perform one shared probe while
    /// concurrent callers wait on the same cache lock.
    pub async fn get_or_probe(
        &self,
        key: &str,
        endpoints: &BTreeMap<String, TapEndpointProbe>,
        timeout: Duration,
        ttl: Duration,
    ) -> TapHealthReport {
        let entry = {
            let mut state = self.state.lock().await;
            state.retain(|_, slot| {
                slot.last_accessed.elapsed() < ttl || Arc::strong_count(&slot.entry) > 1
            });
            if let Some(slot) = state.get_mut(key) {
                slot.last_accessed = Instant::now();
                Arc::clone(&slot.entry)
            } else {
                if state.len() >= MAX_TAP_HEALTH_CACHE_ENTRIES {
                    if let Some(oldest) = state
                        .iter()
                        .filter(|(_, slot)| Arc::strong_count(&slot.entry) == 1)
                        .min_by_key(|(_, slot)| slot.last_accessed)
                        .or_else(|| state.iter().min_by_key(|(_, slot)| slot.last_accessed))
                        .map(|(key, _)| key.clone())
                    {
                        state.remove(&oldest);
                    }
                }
                let entry = Arc::new(TapHealthCacheEntry::default());
                state.insert(
                    key.to_string(),
                    TapHealthCacheSlot {
                        last_accessed: Instant::now(),
                        entry: Arc::clone(&entry),
                    },
                );
                entry
            }
        };

        // Serialize only callers for this revision key. The global cache map
        // is unlocked before provider I/O, so an unrelated project cannot be
        // held behind another project's slow endpoint.
        let mut cached = entry.cached.lock().await;
        if let Some(cached) = cached
            .as_ref()
            .filter(|cached| cached.checked_at.elapsed() < ttl)
        {
            return cached.report.clone();
        }
        let report = probe_tap_health(endpoints, timeout).await;
        *cached = Some(CachedTapHealth {
            checked_at: Instant::now(),
            report: report.clone(),
        });
        report
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TapHealthReport {
    pub endpoints: BTreeMap<String, TapEndpointStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TapEndpointStatus {
    pub configured: bool,
    pub reachable: bool,
}

impl TapHealthReport {
    pub fn endpoint(&self, name: &str) -> TapEndpointStatus {
        self.endpoints
            .get(name)
            .cloned()
            .unwrap_or(TapEndpointStatus {
                configured: false,
                reachable: false,
            })
    }
}

pub fn all_reachable(report: &TapHealthReport, required_adapters: &[String]) -> bool {
    required_adapters.iter().all(|adapter| {
        let endpoint = report.endpoint(adapter);
        endpoint.configured && endpoint.reachable
    })
}

pub fn unreachable_adapters(report: &TapHealthReport, required_adapters: &[String]) -> Vec<String> {
    required_adapters
        .iter()
        .filter(|adapter| {
            let endpoint = report.endpoint(adapter);
            !endpoint.configured || !endpoint.reachable
        })
        .cloned()
        .collect()
}

pub async fn probe_tap_health(
    endpoints: &BTreeMap<String, TapEndpointProbe>,
    timeout: Duration,
) -> TapHealthReport {
    let mut tasks = tokio::task::JoinSet::new();
    for (name, endpoint) in endpoints {
        let name = name.clone();
        let endpoint = endpoint.clone();
        tasks.spawn(async move { (name, probe_endpoint(&endpoint, timeout).await) });
    }
    let mut statuses = BTreeMap::new();
    while let Some(result) = tasks.join_next().await {
        if let Ok((name, status)) = result {
            statuses.insert(name, status);
        }
    }
    TapHealthReport {
        endpoints: statuses,
    }
}

async fn probe_endpoint(endpoint: &TapEndpointProbe, timeout: Duration) -> TapEndpointStatus {
    if endpoint.url.trim().is_empty() {
        return TapEndpointStatus {
            configured: false,
            reachable: false,
        };
    }
    let client = HttpTapAdapter::new(&endpoint.url)
        .with_policy(timeout, 0)
        .with_mode(endpoint.mode);
    TapEndpointStatus {
        configured: true,
        reachable: client.health().await.is_ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn local_tap_with_body(
        status: &str,
        body: &str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_string();
        let body = body.to_string();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0_u8; 8192];
            let read = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..read]).to_string();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            request
        });
        (format!("http://{address}/tap/sync"), task)
    }

    async fn local_tap(status: &str) -> (String, tokio::task::JoinHandle<String>) {
        let body = if status == "200 OK" {
            r#"[{"table_name":"TAP_SCHEMA.tables"}]"#
        } else {
            "unavailable"
        };
        local_tap_with_body(status, body).await
    }

    #[test]
    fn missing_required_adapter_is_unreachable() {
        let report = TapHealthReport::default();
        assert!(!all_reachable(&report, &["catalog".into()]));
        assert_eq!(
            unreachable_adapters(&report, &["catalog".into()]),
            vec!["catalog"]
        );
    }

    #[tokio::test]
    async fn probes_arbitrary_endpoint_with_explicit_mode() {
        let (url, request) = local_tap("200 OK").await;
        let report = probe_tap_health(
            &BTreeMap::from([(
                "catalog".into(),
                TapEndpointProbe {
                    url,
                    mode: TapMode::SyncPost,
                },
            )]),
            Duration::from_secs(2),
        )
        .await;

        assert!(all_reachable(&report, &["catalog".into()]));
        assert!(request.await.unwrap().starts_with("POST "));
    }

    #[tokio::test]
    async fn reports_real_http_failure() {
        let (url, request) = local_tap("503 Service Unavailable").await;
        let report = probe_tap_health(
            &BTreeMap::from([(
                "archive".into(),
                TapEndpointProbe {
                    url,
                    mode: TapMode::SyncGet,
                },
            )]),
            Duration::from_secs(2),
        )
        .await;

        assert!(!all_reachable(&report, &["archive".into()]));
        assert!(request.await.unwrap().starts_with("GET "));
    }

    #[tokio::test]
    async fn empty_health_query_result_is_unreachable() {
        let (url, request) = local_tap_with_body("200 OK", "[]").await;
        let report = probe_tap_health(
            &BTreeMap::from([(
                "archive".into(),
                TapEndpointProbe {
                    url,
                    mode: TapMode::SyncPost,
                },
            )]),
            Duration::from_secs(2),
        )
        .await;

        assert!(!all_reachable(&report, &["archive".into()]));
        assert!(request.await.unwrap().starts_with("POST "));
    }

    #[tokio::test]
    async fn async_endpoint_health_uses_sibling_sync_without_creating_a_job() {
        let (sync_url, request) = local_tap("200 OK").await;
        let async_url = sync_url.replace("/tap/sync", "/tap/async");
        let report = probe_tap_health(
            &BTreeMap::from([(
                "archive".into(),
                TapEndpointProbe {
                    url: async_url,
                    mode: TapMode::AsyncJob,
                },
            )]),
            Duration::from_secs(2),
        )
        .await;

        assert!(all_reachable(&report, &["archive".into()]));
        let request = request.await.unwrap();
        assert!(request.starts_with("POST /tap/sync"), "{request}");
        assert!(!request.contains("/tap/async"), "{request}");
    }

    #[tokio::test]
    async fn concurrent_and_repeated_cache_callers_share_one_provider_probe() {
        let (url, request) = local_tap("200 OK").await;
        let endpoints = BTreeMap::from([(
            "archive".into(),
            TapEndpointProbe {
                url,
                mode: TapMode::SyncPost,
            },
        )]);
        let cache = TapHealthCache::default();
        let timeout = Duration::from_secs(2);
        let ttl = Duration::from_secs(60);

        let (first, second) = tokio::join!(
            cache.get_or_probe("project-revision-1", &endpoints, timeout, ttl),
            cache.get_or_probe("project-revision-1", &endpoints, timeout, ttl),
        );
        let third = cache
            .get_or_probe("project-revision-1", &endpoints, timeout, ttl)
            .await;

        assert!(all_reachable(&first, &["archive".into()]));
        assert_eq!(first, second);
        assert_eq!(first, third);
        assert!(request.await.unwrap().starts_with("POST "));
    }

    #[tokio::test]
    async fn cache_retains_multiple_project_revision_keys() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = tokio::spawn(async move {
            let mut requests = Vec::new();
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![0_u8; 8192];
                let read = socket.read(&mut request).await.unwrap();
                requests.push(String::from_utf8_lossy(&request[..read]).to_string());
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 36\r\nConnection: close\r\n\r\n[{\"table_name\":\"TAP_SCHEMA.tables\"}]",
                    )
                    .await
                    .unwrap();
            }
            requests
        });
        let endpoints = BTreeMap::from([(
            "archive".into(),
            TapEndpointProbe {
                url: format!("http://{address}/tap/sync"),
                mode: TapMode::SyncPost,
            },
        )]);
        let cache = TapHealthCache::default();
        let timeout = Duration::from_secs(2);
        let ttl = Duration::from_secs(60);

        let first = cache
            .get_or_probe("project-a:1", &endpoints, timeout, ttl)
            .await;
        let second = cache
            .get_or_probe("project-b:1", &endpoints, timeout, ttl)
            .await;
        let first_again = cache
            .get_or_probe("project-a:1", &endpoints, timeout, ttl)
            .await;

        assert!(all_reachable(&first, &["archive".into()]));
        assert!(all_reachable(&second, &["archive".into()]));
        assert_eq!(first, first_again);
        assert_eq!(requests.await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn slow_project_key_does_not_block_an_unrelated_key() {
        let slow_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let slow_address = slow_listener.local_addr().unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let slow_server = tokio::spawn(async move {
            let (mut socket, _) = slow_listener.accept().await.unwrap();
            let mut request = vec![0_u8; 8192];
            socket.read(&mut request).await.unwrap();
            started_tx.send(()).unwrap();
            release_rx.await.unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 36\r\nConnection: close\r\n\r\n[{\"table_name\":\"TAP_SCHEMA.tables\"}]",
                )
                .await
                .unwrap();
        });
        let (fast_url, fast_request) = local_tap("200 OK").await;
        let slow_endpoints = BTreeMap::from([(
            "archive".into(),
            TapEndpointProbe {
                url: format!("http://{slow_address}/tap/sync"),
                mode: TapMode::SyncPost,
            },
        )]);
        let fast_endpoints = BTreeMap::from([(
            "catalog".into(),
            TapEndpointProbe {
                url: fast_url,
                mode: TapMode::SyncPost,
            },
        )]);
        let cache = Arc::new(TapHealthCache::default());
        let slow_cache = Arc::clone(&cache);
        let slow_probe = tokio::spawn(async move {
            slow_cache
                .get_or_probe(
                    "project-a:1",
                    &slow_endpoints,
                    Duration::from_secs(2),
                    Duration::from_secs(60),
                )
                .await
        });
        started_rx.await.unwrap();

        let fast_report = tokio::time::timeout(
            Duration::from_millis(500),
            cache.get_or_probe(
                "project-b:1",
                &fast_endpoints,
                Duration::from_secs(2),
                Duration::from_secs(60),
            ),
        )
        .await
        .expect("unrelated key was blocked by slow provider");
        assert!(all_reachable(&fast_report, &["catalog".into()]));

        release_tx.send(()).unwrap();
        assert!(all_reachable(
            &slow_probe.await.unwrap(),
            &["archive".into()]
        ));
        slow_server.await.unwrap();
        assert!(fast_request.await.unwrap().starts_with("POST "));
    }
}
