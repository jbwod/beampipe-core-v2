use crate::{parse_votable_xml, rows_from_json, AdapterError, TapQueryRequest, TapRow};
use reqwest::header::LOCATION;
use serde_json::Value;
use std::time::{Duration, Instant};

pub async fn query_rows_async(
    client: &reqwest::Client,
    base_url: &str,
    adql: &str,
    timeout: Duration,
) -> Result<Vec<TapRow>, AdapterError> {
    let trimmed = base_url.trim_end_matches('/');
    let async_url = if trimmed.ends_with("/async") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/async")
    };
    let request = TapQueryRequest::new(adql);
    let response = client
        .post(&async_url)
        .form(&request.params())
        .timeout(timeout.min(Duration::from_secs(30)))
        .send()
        .await?;
    let job_url = response
        .headers()
        .get(LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .ok_or_else(|| {
            AdapterError::Transient("TAP async submit missing Location header".into())
        })?;
    wait_for_job(client, &job_url, timeout).await?;
    let results = client
        .get(format!("{job_url}/results/result"))
        .timeout(timeout.min(Duration::from_secs(60)))
        .send()
        .await?
        .error_for_status()?;
    parse_tap_body(results).await
}

async fn wait_for_job(
    client: &reqwest::Client,
    job_url: &str,
    timeout: Duration,
) -> Result<(), AdapterError> {
    let started = Instant::now();
    let poll_interval = Duration::from_secs(2);
    loop {
        if started.elapsed() >= timeout {
            return Err(AdapterError::Timeout);
        }
        let phase = fetch_phase(client, job_url).await?;
        match phase.as_str() {
            "COMPLETED" => return Ok(()),
            "ERROR" | "ABORTED" => {
                return Err(AdapterError::Permanent(format!(
                    "TAP async job {phase}: {job_url}"
                )));
            }
            _ => tokio::time::sleep(poll_interval).await,
        }
    }
}

async fn fetch_phase(client: &reqwest::Client, job_url: &str) -> Result<String, AdapterError> {
    let mut last_error = None;
    for _ in 0..3 {
        match client
            .get(format!("{job_url}/phase"))
            .timeout(Duration::from_secs(15))
            .send()
            .await
        {
            Ok(response) => match response.error_for_status() {
                Ok(resp) => return Ok(resp.text().await?.trim().to_string()),
                Err(err) => last_error = Some(AdapterError::Http(err)),
            },
            Err(err) if err.is_timeout() => last_error = Some(AdapterError::Timeout),
            Err(err) if err.is_connect() || err.is_request() => {
                last_error = Some(AdapterError::Transient(err.to_string()))
            }
            Err(err) => return Err(AdapterError::Http(err)),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(last_error.unwrap_or_else(|| AdapterError::Transient("phase poll failed".into())))
}

async fn parse_tap_body(response: reqwest::Response) -> Result<Vec<TapRow>, AdapterError> {
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let text = response.text().await?;
    if content_type.contains("json")
        || text.trim_start().starts_with('{')
        || text.trim_start().starts_with('[')
    {
        let value: Value = serde_json::from_str(&text)
            .map_err(|e| AdapterError::InvalidRowShape(e.to_string()))?;
        return rows_from_json(value);
    }
    if content_type.contains("xml") || text.trim_start().starts_with("<?xml") {
        return parse_votable_xml(&text);
    }
    Err(AdapterError::InvalidRowShape(
        "unsupported TAP async response content type".into(),
    ))
}
