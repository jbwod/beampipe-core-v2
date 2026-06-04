use reqwest::Client;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmProbeResult {
    Ok,
    Unreachable(String),
    NotConfigured,
}

async fn probe_http_base(base_url: &str, timeout: Duration) -> TmProbeResult {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return TmProbeResult::NotConfigured;
    }
    let base = trimmed.trim_end_matches('/');
    let client = match Client::builder().timeout(timeout).build() {
        Ok(c) => c,
        Err(e) => {
            return TmProbeResult::Unreachable(format!("HTTP client build failed: {e}"));
        }
    };
    match client.get(base).send().await {
        Ok(_resp) => TmProbeResult::Ok,
        Err(err) => TmProbeResult::Unreachable(describe_http_connect_error(base, &err)),
    }
}

/// Quick reachability check before expensive staging / graph download.
pub async fn probe_tm_reachable(tm_url: &str, timeout: Duration) -> TmProbeResult {
    probe_http_base(tm_url, timeout).await
}

/// Quick DIM reachability check for rest_remote profiles.
pub async fn probe_dim_reachable(dim_base: &str, timeout: Duration) -> TmProbeResult {
    probe_http_base(dim_base, timeout).await
}

pub fn tm_unreachable_message(tm_url: &str, detail: &str) -> String {
    let host_hint = if tm_url.contains("dlg-tm.desk") || tm_url.contains(".desk") {
        " If using Pawsey desk hostnames, connect VPN or add /etc/hosts entries."
    } else {
        ""
    };
    format!(
        "DALiuGE Translation Manager at {tm_url} is unreachable ({detail}). \
         Start TM (e.g. docker-compose up dlg-tm) before submit.{host_hint}"
    )
}

pub fn dim_unreachable_message(dim_url: &str, detail: &str) -> String {
    format!(
        "DALiuGE DIM at {dim_url} is unreachable ({detail}). \
         Start DIM or verify deploy_host/deploy_port in the deployment profile."
    )
}

pub fn describe_http_connect_error(base_url: &str, err: &reqwest::Error) -> String {
    if err.is_timeout() {
        format!("timed out connecting to {base_url}")
    } else if err.is_connect() {
        format!("connection failed to {base_url}")
    } else if err.is_request() {
        format!("request error for {base_url}: {err}")
    } else {
        format!("{err}")
    }
}

pub fn format_service_request_error(
    service: &str,
    base_url: &str,
    path: &str,
    err: reqwest::Error,
) -> String {
    let endpoint = format!("{base_url}{path}");
    if err.is_connect() || err.is_timeout() {
        format!(
            "{service} request failed ({}) — is the service running and reachable at {base_url}?",
            describe_http_connect_error(&endpoint, &err)
        )
    } else {
        format!("{service} request failed for {endpoint}: {err}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tm_message_mentions_desk_vpn() {
        let msg = tm_unreachable_message("http://dlg-tm.desk", "connection failed");
        assert!(msg.contains("dlg-tm.desk"));
        assert!(msg.contains("VPN"));
    }

    #[test]
    fn unreachable_detail_is_included() {
        let msg = tm_unreachable_message("http://dlg-tm.desk", "connection failed");
        assert!(msg.contains("connection failed"));
        assert!(msg.contains("docker-compose"));
    }

    #[test]
    fn dim_message_mentions_deploy_profile() {
        let msg = dim_unreachable_message("http://dim.local:8000", "connection failed");
        assert!(msg.contains("dim.local:8000"));
        assert!(msg.contains("deploy_host"));
    }
}
