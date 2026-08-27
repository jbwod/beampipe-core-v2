use std::collections::HashMap;

use serde::Deserialize;

pub fn parse_job_results(xml_text: &str) -> (HashMap<String, String>, HashMap<String, String>) {
    let mut data_url_by_scan_id = HashMap::new();
    let mut checksum_url_by_scan_id = HashMap::new();
    for (result_id, url) in iter_uws_results(xml_text) {
        let Some(scan_id) = extract_visibility_scan_id(&result_id) else {
            continue;
        };
        if result_id.contains(".checksum") {
            checksum_url_by_scan_id.insert(scan_id, url);
        } else {
            data_url_by_scan_id.insert(scan_id, url);
        }
    }
    (data_url_by_scan_id, checksum_url_by_scan_id)
}

pub fn parse_eval_job_results(
    xml_text: &str,
) -> (HashMap<String, String>, HashMap<String, String>) {
    let mut eval_url_by_filename = HashMap::new();
    let mut eval_checksum_url_by_filename = HashMap::new();
    for (result_id, url) in iter_uws_results(xml_text) {
        if result_id.contains(".checksum") {
            if let Some(filename) = extract_filename_from_url(&url) {
                let base = filename.strip_suffix(".checksum").unwrap_or(&filename);
                eval_checksum_url_by_filename.insert(base.to_string(), url);
            }
        } else if let Some(filename) = extract_filename_from_url(&url) {
            eval_url_by_filename.insert(filename, url);
        }
    }
    (eval_url_by_filename, eval_checksum_url_by_filename)
}

pub fn extract_scan_id(obs_publisher_did: &str) -> Option<String> {
    obs_publisher_did
        .split("scan-")
        .nth(1)
        .and_then(|rest| rest.split('-').next())
        .map(str::to_string)
}

fn extract_visibility_scan_id(result_id: &str) -> Option<String> {
    result_id
        .strip_prefix("visibility-")
        .and_then(|rest| rest.split('.').next())
        .map(str::to_string)
}

fn extract_filename_from_url(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let encoded = path.rsplit('/').next()?;
    let decoded = percent_decode_path_segment(encoded)?;
    (!decoded.is_empty()).then_some(decoded)
}

fn percent_decode_path_segment(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            decoded.push((hex_digit(high)? << 4) | hex_digit(low)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct UwsResults {
    #[serde(default, rename = "result")]
    results: Vec<UwsResult>,
}

#[derive(Debug, Deserialize)]
struct UwsResult {
    #[serde(default, rename = "@id")]
    id: String,
    #[serde(default, rename = "@href")]
    href: Option<String>,
    #[serde(default)]
    reference: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UwsJob {
    #[serde(default)]
    phase: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UwsPhase {
    #[serde(default, rename = "$text")]
    value: String,
}

pub fn iter_uws_results(xml_text: &str) -> Vec<(String, String)> {
    let Ok(results) = quick_xml::de::from_str::<UwsResults>(xml_text) else {
        tracing::warn!("event=uws_results_parse_error");
        return Vec::new();
    };
    results
        .results
        .into_iter()
        .filter_map(|result| {
            let id = result.id.trim();
            let url = result.href.or(result.reference)?;
            let url = url.trim();
            if id.is_empty() || url.is_empty() {
                None
            } else {
                Some((id.to_owned(), url.to_owned()))
            }
        })
        .collect()
}

/// Parse either a UWS job document, a namespaced phase element, or the plain
/// phase response used by some TAP services.
pub fn parse_uws_phase(xml_text: &str) -> Option<String> {
    let trimmed = xml_text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.starts_with('<') {
        return Some(trimmed.to_owned());
    }
    quick_xml::de::from_str::<UwsJob>(trimmed)
        .ok()
        .and_then(|job| job.phase)
        .or_else(|| {
            quick_xml::de::from_str::<UwsPhase>(trimmed)
                .ok()
                .map(|phase| phase.value)
        })
        .map(|phase| phase.trim().to_owned())
        .filter(|phase| !phase.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_visibility_scan_ids() {
        let xml = include_str!("../tests/fixtures/uws_results_namespaced.xml");
        let (data, checksum) = parse_job_results(xml);
        assert_eq!(data.get("105174").unwrap(), "https://example/a");
        assert_eq!(checksum.get("105174").unwrap(), "https://example/cs");
    }

    #[test]
    fn parses_real_uws_xlink_results_and_decodes_filenames() {
        let xml = r#"<?xml version="1.0"?>
        <uws:results xmlns:uws="http://www.ivoa.net/xml/UWS/v1.0"
                     xmlns:xlink="http://www.w3.org/1999/xlink">
          <uws:result id="evaluation-90293"
            xlink:href="https://example.test/cache/calibration-metadata-processing-logs-SB72962_2025-04-21-063210.tar?token=redacted"/>
          <uws:result id="evaluation-90293.checksum"
            xlink:href="https://example.test/cache/calibration-metadata-processing-logs-SB72962_2025-04-21-063210.tar.checksum?token=redacted"/>
          <uws:result id="visibility-644741"
            xlink:href="https://example.test/cache/HIPASSJ1317-16%5FSB72962.ms.tar"/>
        </uws:results>"#;
        let results = iter_uws_results(xml);
        assert_eq!(results.len(), 3);
        let (eval, checksums) = parse_eval_job_results(xml);
        let filename = "calibration-metadata-processing-logs-SB72962_2025-04-21-063210.tar";
        assert!(eval[filename].contains(filename));
        assert!(checksums[filename].contains(".checksum"));
        let (visibilities, _) = parse_job_results(xml);
        assert!(visibilities["644741"].contains("HIPASSJ1317-16"));
    }

    #[test]
    fn parses_plain_and_namespaced_uws_phases() {
        assert_eq!(parse_uws_phase(" COMPLETED ").as_deref(), Some("COMPLETED"));
        assert_eq!(
            parse_uws_phase(
                r#"<u:job xmlns:u="http://www.ivoa.net/xml/UWS/v1.1"><u:phase>EXECUTING</u:phase></u:job>"#
            )
            .as_deref(),
            Some("EXECUTING")
        );
        assert_eq!(
            parse_uws_phase(
                r#"<phase xmlns="http://www.ivoa.net/xml/UWS/v1.1">COMPLETED</phase>"#
            )
            .as_deref(),
            Some("COMPLETED")
        );
    }
}
