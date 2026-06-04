use std::collections::HashMap;

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
    url.rsplit('/').next().map(str::to_string)
}

pub fn iter_uws_results(xml_text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut reader = quick_xml::Reader::from_str(xml_text);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut current_id = String::new();
    let mut in_reference = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name == "uws:result" || name == "result" {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"id" {
                            current_id = String::from_utf8_lossy(&attr.value).to_string();
                        }
                    }
                }
                if name == "uws:reference" || name == "reference" {
                    in_reference = true;
                }
            }
            Ok(quick_xml::events::Event::Text(e)) if in_reference => {
                let url = e.unescape().unwrap_or_default().to_string();
                if !current_id.is_empty() && !url.is_empty() {
                    out.push((current_id.clone(), url));
                }
                in_reference = false;
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_visibility_scan_ids() {
        let xml = r#"<?xml version="1.0"?>
        <uws:results xmlns:uws="http://www.ivoa.net/xml/UWS/v1.0">
          <uws:result id="visibility-105174"><uws:reference>https://example/a</uws:reference></uws:result>
          <uws:result id="visibility-105174.checksum"><uws:reference>https://example/cs</uws:reference></uws:result>
        </uws:results>"#;
        let (data, checksum) = parse_job_results(xml);
        assert_eq!(data.get("105174").unwrap(), "https://example/a");
        assert_eq!(checksum.get("105174").unwrap(), "https://example/cs");
    }
}
