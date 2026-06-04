use crate::{casda_tap, vizier_tap, TapClient};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TapHealthReport {
    pub casda: TapEndpointStatus,
    pub vizier: TapEndpointStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TapEndpointStatus {
    pub configured: bool,
    pub reachable: bool,
}

impl TapHealthReport {
    pub fn empty() -> Self {
        Self {
            casda: TapEndpointStatus {
                configured: false,
                reachable: true,
            },
            vizier: TapEndpointStatus {
                configured: false,
                reachable: true,
            },
        }
    }
}

pub fn all_reachable(report: &TapHealthReport, required_adapters: &[String]) -> bool {
    for adapter in required_adapters {
        match adapter.as_str() {
            "casda" if report.casda.configured && !report.casda.reachable => return false,
            "vizier" if report.vizier.configured && !report.vizier.reachable => return false,
            _ => {}
        }
    }
    true
}

pub fn unreachable_adapters(report: &TapHealthReport, required_adapters: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for adapter in required_adapters {
        match adapter.as_str() {
            "casda" if report.casda.configured && !report.casda.reachable => {
                out.push("casda".into())
            }
            "vizier" if report.vizier.configured && !report.vizier.reachable => {
                out.push("vizier".into())
            }
            _ => {}
        }
    }
    out
}

pub async fn probe_tap_health(
    casda_url: Option<&str>,
    vizier_url: Option<&str>,
    timeout: Duration,
) -> TapHealthReport {
    let casda = probe_endpoint(casda_url, timeout).await;
    let vizier = probe_endpoint(vizier_url, timeout).await;
    TapHealthReport { casda, vizier }
}

async fn probe_endpoint(url: Option<&str>, timeout: Duration) -> TapEndpointStatus {
    let Some(url) = url.filter(|u| !u.trim().is_empty()) else {
        return TapEndpointStatus {
            configured: false,
            reachable: true,
        };
    };
    let client = if url.contains("vizier") {
        vizier_tap(url.to_string()).with_policy(timeout, 0)
    } else {
        casda_tap(url.to_string()).with_policy(timeout, 0)
    };
    TapEndpointStatus {
        configured: true,
        reachable: client.health().await.is_ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_unconfigured_adapters() {
        let report = TapHealthReport {
            casda: TapEndpointStatus {
                configured: false,
                reachable: false,
            },
            vizier: TapEndpointStatus {
                configured: true,
                reachable: true,
            },
        };
        assert!(all_reachable(&report, &["casda".into(), "vizier".into()]));
    }

    #[test]
    fn flags_unreachable_required() {
        let report = TapHealthReport {
            casda: TapEndpointStatus {
                configured: true,
                reachable: false,
            },
            vizier: TapEndpointStatus {
                configured: false,
                reachable: true,
            },
        };
        assert!(!all_reachable(&report, &["casda".into()]));
        assert_eq!(
            unreachable_adapters(&report, &["casda".into()]),
            vec!["casda"]
        );
    }
}
