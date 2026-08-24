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
    let create_status = response.status();
    let location = response
        .headers()
        .get(LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if !uws_status_accepted(create_status) {
        return Err(uws_status_error("create", response).await);
    }
    let location = location.ok_or_else(|| {
        AdapterError::Transient("TAP async submit missing Location header".into())
    })?;
    let join_base = reqwest::Url::parse(&format!("{async_url}/")).map_err(|error| {
        AdapterError::InvalidRowShape(format!("invalid TAP async endpoint URL: {error}"))
    })?;
    let job_url = join_base.join(&location).map_err(|error| {
        AdapterError::InvalidRowShape(format!("invalid TAP async job Location: {error}"))
    })?;
    let job_url = job_url.as_str().trim_end_matches('/').to_string();

    let run_response = client
        .post(format!("{job_url}/phase"))
        .form(&[("PHASE", "RUN")])
        .timeout(timeout.min(Duration::from_secs(30)))
        .send()
        .await;
    let run_response = match run_response {
        Ok(response) => response,
        Err(error) => {
            best_effort_cleanup(client, &job_url, true).await;
            return Err(AdapterError::Http(error));
        }
    };
    if !uws_status_accepted(run_response.status()) {
        let error = uws_status_error("start", run_response).await;
        best_effort_cleanup(client, &job_url, true).await;
        return Err(error);
    }

    if let Err(error) = wait_for_job(client, &job_url, timeout).await {
        best_effort_cleanup(client, &job_url, true).await;
        return Err(error);
    }
    let result = client
        .get(format!("{job_url}/results/result"))
        .timeout(timeout.min(Duration::from_secs(60)))
        .send()
        .await;
    let result = match result {
        Ok(response) => match response.error_for_status() {
            Ok(response) => parse_tap_body(response).await,
            Err(error) => Err(AdapterError::Http(error)),
        },
        Err(error) => Err(AdapterError::Http(error)),
    };
    best_effort_cleanup(client, &job_url, false).await;
    result
}

fn uws_status_accepted(status: reqwest::StatusCode) -> bool {
    status.is_success() || status.is_redirection()
}

async fn uws_status_error(step: &str, response: reqwest::Response) -> AdapterError {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let detail = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let detail = detail.chars().take(240).collect::<String>();
    let message = format!("TAP async {step} failed with HTTP {status}: {detail}");
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        AdapterError::Transient(message)
    } else {
        AdapterError::Permanent(message)
    }
}

async fn best_effort_cleanup(client: &reqwest::Client, job_url: &str, abort: bool) {
    let cleanup_timeout = Duration::from_secs(5);
    if abort {
        let _ = client
            .post(format!("{job_url}/phase"))
            .form(&[("PHASE", "ABORT")])
            .timeout(cleanup_timeout)
            .send()
            .await;
    }
    let _ = client.delete(job_url).timeout(cleanup_timeout).send().await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_request(socket: &mut TcpStream) -> String {
        let mut request = Vec::new();
        loop {
            let mut chunk = [0_u8; 2048];
            let read = socket.read(&mut chunk).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..read]);
            let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
                continue;
            };
            let header_end = header_end + 4;
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if request.len() >= header_end + content_length {
                break;
            }
        }
        String::from_utf8_lossy(&request).to_string()
    }

    fn response(status: &str, extra_headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn async_job_lifecycle_creates_starts_polls_reads_and_deletes() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let responses = [
                response("303 See Other", "Location: /tap/async/42\r\n", ""),
                response("303 See Other", "", ""),
                response("200 OK", "Content-Type: text/plain\r\n", "COMPLETED"),
                response(
                    "200 OK",
                    "Content-Type: application/json\r\n",
                    r#"[{"table_name":"TAP_SCHEMA.tables"}]"#,
                ),
                response("200 OK", "", ""),
            ];
            let mut requests = Vec::new();
            for response in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                requests.push(read_request(&mut socket).await);
                socket.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();

        let rows = query_rows_async(
            &client,
            &format!("http://{address}/tap/async"),
            "SELECT TOP 1 table_name FROM TAP_SCHEMA.tables",
            Duration::from_secs(2),
        )
        .await
        .unwrap();

        assert_eq!(rows.len(), 1);
        let requests = server.await.unwrap();
        assert!(requests[0].starts_with("POST /tap/async "));
        assert!(requests[1].starts_with("POST /tap/async/42/phase "));
        assert!(requests[1].contains("PHASE=RUN"));
        assert!(requests[2].starts_with("GET /tap/async/42/phase "));
        assert!(requests[3].starts_with("GET /tap/async/42/results/result "));
        assert!(requests[4].starts_with("DELETE /tap/async/42 "));
    }
}
