use crate::{OrchestrationError, StageOutcome, StagingClient};
use async_trait::async_trait;
use beampipe_adapters::{
    extract_scan_id, parse_casda_datalink, parse_eval_job_results, parse_job_results,
};
use reqwest::Client;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;
use tracing::debug;

const CASDA_ASYNC_SERVICE: &str = "async_service";
const DEFAULT_CASDA_LOGIN_URL: &str = "https://data.csiro.au/casda_vo_proxy/vo/tap/availability";

#[derive(Debug, Clone)]
pub struct CasdaStagingClient {
    pub username: String,
    pub password: String,
    pub login_url: String,
    pub client: Client,
    pub stage_by_sbid: bool,
}

impl CasdaStagingClient {
    pub fn from_env() -> Option<Self> {
        let username = std::env::var("CASDA_USERNAME").ok()?;
        let password = std::env::var("CASDA_PASSWORD").ok()?;
        let login_url = std::env::var("CASDA_LOGIN_URL")
            .unwrap_or_else(|_| DEFAULT_CASDA_LOGIN_URL.to_string());
        Some(Self {
            username,
            password,
            login_url,
            client: Client::builder()
                .cookie_store(true)
                .timeout(Duration::from_secs(120))
                .build()
                .ok()?,
            stage_by_sbid: std::env::var("CASDA_STAGE_BY_SBID")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(true),
        })
    }

    async fn authenticate(&self) -> Result<(), String> {
        let resp = self
            .client
            .get(&self.login_url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!(
                "CASDA login failed: HTTP {} ({})",
                resp.status(),
                self.login_url
            ))
        }
    }

    /// Preflight: login and establish session cookies before staging.
    pub async fn verify_credentials(&self) -> Result<(), String> {
        self.authenticate().await
    }

    fn sort_sbids(sbids: impl IntoIterator<Item = String>) -> Vec<String> {
        let mut items: Vec<(i64, String)> = sbids
            .into_iter()
            .map(|s| (s.parse::<i64>().unwrap_or(i64::MAX), s))
            .collect();
        items.sort_by_key(|(n, _)| *n);
        items.into_iter().map(|(_, s)| s).collect()
    }
}

#[async_trait]
impl StagingClient for CasdaStagingClient {
    async fn stage(&self, metadata: &[Value]) -> Result<StageOutcome, OrchestrationError> {
        if metadata.is_empty() {
            return Ok(StageOutcome::default());
        }
        self.authenticate()
            .await
            .map_err(OrchestrationError::Backend)?;
        let mut staged_metadata = Vec::new();
        let mut skipped = Vec::new();
        let mut staged_urls = HashMap::new();
        let mut checksum_urls = HashMap::new();
        let mut eval_urls = HashMap::new();
        let mut eval_checksum_urls = HashMap::new();

        let batches: BTreeMap<String, Vec<Value>> = if self.stage_by_sbid {
            let mut by_sbid: BTreeMap<String, Vec<Value>> = BTreeMap::new();
            for rec in metadata {
                let sbid = rec
                    .get("sbid")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                by_sbid.entry(sbid).or_default().push(rec.clone());
            }
            by_sbid
        } else {
            let mut map = BTreeMap::new();
            map.insert("combined".into(), metadata.to_vec());
            map
        };

        let sbid_order: Vec<String> = if self.stage_by_sbid {
            Self::sort_sbids(batches.keys().cloned())
        } else {
            batches.keys().cloned().collect()
        };

        for sbid in sbid_order {
            let Some(records) = batches.get(&sbid) else {
                continue;
            };
            match self.stage_visibility_batch(records).await {
                Ok((data, checksum)) => {
                    staged_urls.extend(data);
                    checksum_urls.extend(checksum);
                    staged_metadata.extend(records.clone());
                }
                Err(err) => {
                    if err.contains("do not have access") {
                        debug!(sbid = %sbid, "event=casda_stage_access_denied");
                        skipped.push(sbid);
                    } else {
                        return Err(OrchestrationError::Backend(err));
                    }
                }
            }
        }

        if let Ok((eval_data, eval_checksum)) = self.stage_eval_batch(metadata).await {
            eval_urls.extend(eval_data);
            eval_checksum_urls.extend(eval_checksum);
        }

        apply_url_maps(
            &mut staged_metadata,
            &staged_urls,
            &checksum_urls,
            &eval_urls,
            &eval_checksum_urls,
        );

        Ok(StageOutcome {
            staged_count: staged_metadata.len(),
            metadata: staged_metadata,
            skipped_sbids: skipped,
            staged_urls_by_scan_id: staged_urls,
            checksum_urls_by_scan_id: checksum_urls,
            eval_urls_by_sbid: eval_urls,
            eval_checksum_urls_by_sbid: eval_checksum_urls,
        })
    }
}

impl CasdaStagingClient {
    async fn stage_visibility_batch(
        &self,
        records: &[Value],
    ) -> Result<(HashMap<String, String>, HashMap<String, String>), String> {
        let access_urls = collect_access_urls(records, &["access_url"]);
        if access_urls.is_empty() {
            return Err("no access_url in metadata for CASDA visibility staging".into());
        }
        let xml = self.create_and_run_soda_job(&access_urls).await?;
        Ok(parse_job_results(&xml))
    }

    async fn stage_eval_batch(
        &self,
        records: &[Value],
    ) -> Result<(HashMap<String, String>, HashMap<String, String>), String> {
        let mut seen = HashSet::new();
        let mut access_urls = Vec::new();
        for rec in records {
            let eval_file = rec.get("evaluation_file").and_then(Value::as_str);
            let sbid = rec.get("sbid").and_then(Value::as_str);
            let access_url = rec
                .get("evaluation_file_access_url")
                .or_else(|| rec.get("access_url"))
                .and_then(Value::as_str);
            if let (Some(file), Some(sbid), Some(url)) = (eval_file, sbid, access_url) {
                let key = (sbid.to_string(), file.to_string());
                if seen.insert(key) {
                    access_urls.push(url.to_string());
                }
            }
        }
        if access_urls.is_empty() {
            return Ok((HashMap::new(), HashMap::new()));
        }
        let xml = self.create_and_run_soda_job(&access_urls).await?;
        let (by_filename, by_filename_cs) = parse_eval_job_results(&xml);
        let mut by_sbid = HashMap::new();
        let mut by_sbid_cs = HashMap::new();
        for rec in records {
            let sbid = rec.get("sbid").and_then(Value::as_str).unwrap_or_default();
            let eval_file = rec
                .get("evaluation_file")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if let Some(url) = by_filename.get(eval_file) {
                by_sbid.insert(sbid.to_string(), url.clone());
            }
            if let Some(url) = by_filename_cs.get(eval_file) {
                by_sbid_cs.insert(sbid.to_string(), url.clone());
            }
        }
        Ok((by_sbid, by_sbid_cs))
    }

    async fn create_and_run_soda_job(&self, access_urls: &[String]) -> Result<String, String> {
        let job_url = self.create_soda_job(access_urls).await?;
        self.run_soda_job(&job_url).await
    }

    async fn create_soda_job(&self, access_urls: &[String]) -> Result<String, String> {
        let mut tokens = Vec::new();
        let mut soda_url = None;
        for access_url in access_urls {
            let (async_url, token) = self.resolve_datalink_token(access_url).await?;
            if let Some(existing) = &soda_url {
                if existing != &async_url {
                    return Err("CASDA datalink returned mismatched async service URLs".into());
                }
            } else {
                soda_url = Some(async_url);
            }
            tokens.push(token);
        }
        let soda_url = soda_url.ok_or_else(|| "no CASDA access URLs to stage".to_string())?;
        let id_params: Vec<(&str, &str)> =
            tokens.iter().map(|token| ("ID", token.as_str())).collect();
        let resp = self
            .client
            .post(&soda_url)
            .query(&id_params)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() && !resp.status().is_redirection() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if text.contains("do not have access") {
                return Err(text);
            }
            return Err(format!("CASDA staging create failed: HTTP {status}"));
        }
        resp.headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .or_else(|| Some(resp.url().to_string()))
            .ok_or_else(|| "CASDA staging missing job URL".to_string())
    }

    async fn resolve_datalink_token(&self, access_url: &str) -> Result<(String, String), String> {
        let resp = self
            .client
            .get(access_url)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!(
                "CASDA datalink request failed: HTTP {}",
                resp.status()
            ));
        }
        let text = resp.text().await.map_err(|e| e.to_string())?;
        parse_casda_datalink(&text, CASDA_ASYNC_SERVICE).ok_or_else(|| {
            format!("CASDA datalink missing {CASDA_ASYNC_SERVICE} token for {access_url}")
        })
    }

    async fn run_soda_job(&self, job_url: &str) -> Result<String, String> {
        self.client
            .post(format!("{job_url}/phase"))
            .form(&[("phase", "RUN")])
            .send()
            .await
            .map_err(|e| e.to_string())?;

        for _ in 0..60 {
            let poll = self
                .client
                .get(job_url)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let body = poll.text().await.map_err(|e| e.to_string())?;
            match read_job_phase(&body) {
                Some(phase) if phase == "COMPLETED" => {
                    let results_url = format!("{job_url}/results");
                    return self
                        .client
                        .get(&results_url)
                        .send()
                        .await
                        .map_err(|e| e.to_string())?
                        .text()
                        .await
                        .map_err(|e| e.to_string());
                }
                Some(phase) if matches!(phase.as_str(), "ERROR" | "ABORTED") => {
                    return Err(format!("CASDA staging job ended with status {phase}"));
                }
                _ => {}
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Err("CASDA staging job timed out".into())
    }
}

fn collect_access_urls(records: &[Value], fields: &[&str]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for rec in records {
        for field in fields {
            if let Some(url) = rec.get(*field).and_then(Value::as_str) {
                if !url.is_empty() && seen.insert(url.to_string()) {
                    out.push(url.to_string());
                }
            }
        }
    }
    out
}

fn read_job_phase(xml: &str) -> Option<String> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut in_phase = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e))
                if e.name().local_name().as_ref() == b"phase" =>
            {
                in_phase = true;
            }
            Ok(quick_xml::events::Event::Text(e)) if in_phase => {
                return Some(e.unescape().unwrap_or_default().trim().to_string());
            }
            Ok(quick_xml::events::Event::End(e)) if e.name().local_name().as_ref() == b"phase" => {
                in_phase = false;
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    None
}

fn apply_url_maps(
    metadata: &mut [Value],
    staged_urls: &HashMap<String, String>,
    checksum_urls: &HashMap<String, String>,
    eval_urls: &HashMap<String, String>,
    eval_checksum_urls: &HashMap<String, String>,
) {
    for rec in metadata.iter_mut() {
        let Some(obj) = rec.as_object_mut() else {
            continue;
        };
        let scan_id = obj
            .get("scan_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                obj.get("obs_publisher_did")
                    .and_then(Value::as_str)
                    .and_then(extract_scan_id)
            });
        if let Some(scan_id) = scan_id {
            if let Some(url) = staged_urls.get(&scan_id) {
                obj.insert("staged_url".into(), Value::String(url.clone()));
            }
            if let Some(url) = checksum_urls.get(&scan_id) {
                let dataset_name = obj
                    .get("dataset_id")
                    .or_else(|| obj.get("name"))
                    .or_else(|| obj.get("visibility_filename"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if url.contains(dataset_name) || dataset_name.is_empty() {
                    obj.insert("checksum_url".into(), Value::String(url.clone()));
                } else {
                    obj.insert("checksum_url".into(), Value::String(String::new()));
                }
            }
        }
        if let Some(sbid) = obj.get("sbid").and_then(Value::as_str).map(str::to_string) {
            if let Some(url) = eval_urls.get(&sbid) {
                obj.insert("evaluation_file_url".into(), Value::String(url.clone()));
            }
            if let Some(url) = eval_checksum_urls.get(&sbid) {
                obj.insert(
                    "evaluation_file_checksum_url".into(),
                    Value::String(url.clone()),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_completed_job_phase() {
        let xml = r#"<?xml version="1.0"?><uws:job xmlns:uws="http://www.ivoa.net/xml/UWS/v1.0"><uws:phase>COMPLETED</uws:phase></uws:job>"#;
        assert_eq!(read_job_phase(xml).as_deref(), Some("COMPLETED"));
    }
}
