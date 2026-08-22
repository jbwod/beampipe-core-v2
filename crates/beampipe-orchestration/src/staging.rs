use crate::{OrchestrationError, StageOutcome, StagingClient};
use async_trait::async_trait;
use beampipe_adapters::{
    extract_scan_id, parse_casda_datalink, parse_eval_job_results, parse_job_results,
};
use beampipe_security::{resolve_secret, SecretPolicy, SecretRef};
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

/// Password from `CASDA_PASSWORD_FILE` when that path is non-empty, else `CASDA_PASSWORD`.
/// An empty `CASDA_PASSWORD_FILE=` in Compose `env_file` is treated as unset.
pub fn casda_password_from_env() -> Option<String> {
    match std::env::var("CASDA_PASSWORD_FILE") {
        Ok(path) if !path.trim().is_empty() => resolve_secret(
            &SecretRef::File { file: path },
            SecretPolicy::from_runtime_env(),
        )
        .ok()
        .map(|secret| secret.expose().to_string()),
        _ => std::env::var("CASDA_PASSWORD")
            .ok()
            .filter(|value| !value.trim().is_empty()),
    }
}

impl CasdaStagingClient {
    pub fn from_env() -> Option<Self> {
        let username = std::env::var("CASDA_USERNAME")
            .ok()
            .filter(|value| !value.is_empty())?;
        let password = casda_password_from_env()?;
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
        let eval_inputs =
            evaluation_staging_inputs(metadata).map_err(OrchestrationError::Backend)?;
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

        let (eval_data, eval_checksum) = self
            .stage_eval_batch(&eval_inputs)
            .await
            .map_err(OrchestrationError::Backend)?;
        eval_urls.extend(eval_data);
        eval_checksum_urls.extend(eval_checksum);

        apply_url_maps(
            &mut staged_metadata,
            &staged_urls,
            &checksum_urls,
            &eval_urls,
            &eval_checksum_urls,
        )
        .map_err(OrchestrationError::Backend)?;

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
        inputs: &BTreeMap<String, (String, String)>,
    ) -> Result<(HashMap<String, String>, HashMap<String, String>), String> {
        let mut seen_urls = HashSet::new();
        let mut access_urls = Vec::new();
        for (_, access_url) in inputs.values() {
            if seen_urls.insert(access_url.clone()) {
                access_urls.push(access_url.clone());
            }
        }
        let xml = self.create_and_run_soda_job(&access_urls).await?;
        let (by_filename, by_filename_cs) = parse_eval_job_results(&xml);
        map_eval_staging_results(inputs, &by_filename, &by_filename_cs)
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

fn evaluation_staging_inputs(
    records: &[Value],
) -> Result<BTreeMap<String, (String, String)>, String> {
    let mut inputs = BTreeMap::new();
    for record in records {
        let sbid = record
            .get("sbid")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "CASDA evaluation staging metadata is missing sbid".to_string())?;
        let filename = record
            .get("evaluation_file")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("SBID {sbid} is missing evaluation_file"))?;
        let expected_prefix = format!("calibration-metadata-processing-logs-SB{sbid}_");
        if !filename.starts_with(&expected_prefix) || !filename.ends_with(".tar") {
            return Err(format!(
                "SBID {sbid} evaluation_file is not a calibration metadata archive"
            ));
        }
        let access_url = record
            .get("evaluation_file_access_url")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("SBID {sbid} is missing evaluation_file_access_url"))?;
        let input = (filename.to_string(), access_url.to_string());
        if let Some(existing) = inputs.get(sbid) {
            if existing != &input {
                return Err(format!(
                    "SBID {sbid} has inconsistent evaluation staging metadata"
                ));
            }
        } else {
            inputs.insert(sbid.to_string(), input);
        }
    }
    if inputs.is_empty() {
        return Err("no calibration evaluation archives in CASDA staging metadata".into());
    }
    Ok(inputs)
}

fn map_eval_staging_results(
    inputs: &BTreeMap<String, (String, String)>,
    by_filename: &HashMap<String, String>,
    by_filename_checksum: &HashMap<String, String>,
) -> Result<(HashMap<String, String>, HashMap<String, String>), String> {
    let mut by_sbid = HashMap::new();
    let mut by_sbid_checksum = HashMap::new();
    for (sbid, (filename, _)) in inputs {
        let staged_url = by_filename.get(filename).ok_or_else(|| {
            format!("CASDA evaluation staging result is missing {filename} for SBID {sbid}")
        })?;
        let checksum_url = by_filename_checksum.get(filename).ok_or_else(|| {
            format!(
                "CASDA evaluation staging result is missing the checksum for {filename} (SBID {sbid})"
            )
        })?;
        by_sbid.insert(sbid.clone(), staged_url.clone());
        by_sbid_checksum.insert(sbid.clone(), checksum_url.clone());
    }
    Ok((by_sbid, by_sbid_checksum))
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
) -> Result<(), String> {
    for rec in metadata.iter_mut() {
        let obj = rec
            .as_object_mut()
            .ok_or_else(|| "CASDA staged metadata record is not an object".to_string())?;
        let scan_id = obj
            .get("scan_id")
            .and_then(Value::as_str)
            .map(|value| extract_scan_id(value).unwrap_or_else(|| value.to_string()))
            .or_else(|| {
                obj.get("obs_publisher_did")
                    .and_then(Value::as_str)
                    .and_then(extract_scan_id)
            })
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "CASDA staged metadata record is missing scan_id".to_string())?;
        let sbid = obj
            .get("sbid")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "CASDA staged metadata record is missing sbid".to_string())?;
        let dataset_name = obj
            .get("dataset_id")
            .or_else(|| obj.get("name"))
            .or_else(|| obj.get("visibility_filename"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                format!("CASDA staged metadata for scan {scan_id} has no dataset name")
            })?;

        let staged_url = required_http_map_value(staged_urls, &scan_id, "visibility data")?;
        let checksum_url = required_http_map_value(checksum_urls, &scan_id, "visibility checksum")?;
        if !checksum_url.contains(dataset_name) {
            return Err(format!(
                "CASDA visibility checksum for scan {scan_id} does not match dataset {dataset_name}"
            ));
        }
        let eval_url = required_http_map_value(eval_urls, sbid, "evaluation archive")?;
        let eval_checksum_url =
            required_http_map_value(eval_checksum_urls, sbid, "evaluation checksum")?;

        obj.insert("staged_url".into(), Value::String(staged_url));
        obj.insert("checksum_url".into(), Value::String(checksum_url));
        obj.insert("evaluation_file_url".into(), Value::String(eval_url));
        obj.insert(
            "evaluation_file_checksum_url".into(),
            Value::String(eval_checksum_url),
        );
    }
    Ok(())
}

fn required_http_map_value(
    values: &HashMap<String, String>,
    key: &str,
    label: &str,
) -> Result<String, String> {
    let value = values
        .get(key)
        .filter(|value| value.starts_with("https://") || value.starts_with("http://"))
        .ok_or_else(|| format!("CASDA staging result is missing HTTP(S) {label} URL for {key}"))?;
    Ok(value.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_completed_job_phase() {
        let xml = r#"<?xml version="1.0"?><uws:job xmlns:uws="http://www.ivoa.net/xml/UWS/v1.0"><uws:phase>COMPLETED</uws:phase></uws:job>"#;
        assert_eq!(read_job_phase(xml).as_deref(), Some("COMPLETED"));
    }

    #[test]
    fn evaluation_staging_requires_explicit_eval_access_url() {
        let error = evaluation_staging_inputs(&[serde_json::json!({
            "sbid": "72962",
            "evaluation_file": "calibration-metadata-processing-logs-SB72962_2025-04-21-063210.tar",
            "access_url": "https://example.test/visibility"
        })])
        .unwrap_err();

        assert_eq!(error, "SBID 72962 is missing evaluation_file_access_url");
    }

    #[test]
    fn staged_metadata_requires_complete_visibility_and_evaluation_evidence() {
        let mut metadata = vec![serde_json::json!({
            "sbid": "72962",
            "scan_id": "scan-9",
            "dataset_id": "HIPASSJ1317-16_SB72962.ms.tar"
        })];
        let data = HashMap::from([(
            "9".into(),
            "https://example.test/HIPASSJ1317-16_SB72962.ms.tar".into(),
        )]);
        let checksum = HashMap::from([(
            "9".into(),
            "https://example.test/HIPASSJ1317-16_SB72962.ms.tar.checksum".into(),
        )]);
        let eval = HashMap::from([(
            "72962".into(),
            "https://example.test/calibration-SB72962.tar".into(),
        )]);
        let eval_checksum = HashMap::from([(
            "72962".into(),
            "https://example.test/calibration-SB72962.tar.checksum".into(),
        )]);

        apply_url_maps(&mut metadata, &data, &checksum, &eval, &eval_checksum).unwrap();
        assert!(metadata[0]["staged_url"]
            .as_str()
            .unwrap()
            .starts_with("https://"));

        let error = apply_url_maps(&mut metadata, &data, &HashMap::new(), &eval, &eval_checksum)
            .unwrap_err();
        assert!(error.contains("visibility checksum"));
    }

    #[tokio::test]
    async fn stage_propagates_eval_preflight_error_before_authentication() {
        let client = CasdaStagingClient {
            username: "unused".into(),
            password: "unused".into(),
            login_url: "http://127.0.0.1:1/must-not-be-called".into(),
            client: Client::new(),
            stage_by_sbid: true,
        };
        let error = client
            .stage(&[serde_json::json!({
                "sbid": "72962",
                "evaluation_file": "calibration-metadata-processing-logs-SB72962_2025-04-21-063210.tar",
                "access_url": "https://example.test/visibility"
            })])
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            OrchestrationError::Backend(message)
                if message == "SBID 72962 is missing evaluation_file_access_url"
        ));
    }

    #[test]
    fn evaluation_staging_rejects_non_calibration_archive() {
        let error = evaluation_staging_inputs(&[serde_json::json!({
            "sbid": "72962",
            "evaluation_file": "WALLABY-validation-SB72962.cube.MilkyWay.tar",
            "evaluation_file_access_url": "https://example.test/validation"
        })])
        .unwrap_err();

        assert_eq!(
            error,
            "SBID 72962 evaluation_file is not a calibration metadata archive"
        );
    }

    #[test]
    fn evaluation_staging_results_must_cover_every_expected_archive() {
        let filename = "calibration-metadata-processing-logs-SB72962_2025-04-21-063210.tar";
        let inputs = evaluation_staging_inputs(&[serde_json::json!({
            "sbid": "72962",
            "evaluation_file": filename,
            "evaluation_file_access_url": "https://example.test/calibration"
        })])
        .unwrap();

        let error =
            map_eval_staging_results(&inputs, &HashMap::new(), &HashMap::new()).unwrap_err();
        assert!(error.contains("CASDA evaluation staging result is missing"));

        let staged = HashMap::from([(
            filename.to_string(),
            "https://example.test/staged-calibration".to_string(),
        )]);
        let error = map_eval_staging_results(&inputs, &staged, &HashMap::new()).unwrap_err();
        assert!(error.contains("missing the checksum"));
    }
}
