use std::collections::HashMap;

/// Parse a CASDA datalink VOTable and return the SODA async URL plus ID tokens for `service_name`.
pub fn parse_casda_datalink(xml: &str, service_name: &str) -> Option<(String, String)> {
    let fields = extract_field_names(xml);
    if fields.is_empty() {
        return None;
    }
    let soda_url = extract_soda_access_url(xml, service_name)?;
    for row in extract_table_rows(&fields, xml) {
        let service = row.get("service_def").map(String::as_str).unwrap_or("");
        if service != service_name {
            continue;
        }
        let token = row
            .get("authenticated_id_token")
            .filter(|t| !t.trim().is_empty())
            .cloned();
        if let Some(token) = token {
            return Some((soda_url, token));
        }
    }
    None
}

pub fn extract_field_names(xml: &str) -> Vec<String> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut names = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Empty(e)) | Ok(quick_xml::events::Event::Start(e))
                if e.name().local_name().as_ref() == b"FIELD" =>
            {
                for attr in e.attributes().flatten() {
                    if attr.key.local_name().as_ref() == b"name" {
                        names.push(String::from_utf8_lossy(&attr.value).to_string());
                    }
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    names
}

fn extract_soda_access_url(xml: &str, service_name: &str) -> Option<String> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut in_target_resource = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e)) => {
                let name = e.name().local_name();
                if name.as_ref() == b"RESOURCE" {
                    in_target_resource = e.attributes().flatten().any(|a| {
                        a.key.local_name().as_ref() == b"ID" && {
                            String::from_utf8_lossy(&a.value) == service_name
                        }
                    });
                } else if in_target_resource && name.as_ref() == b"PARAM" {
                    let mut param_name = None;
                    let mut param_value = None;
                    for attr in e.attributes().flatten() {
                        match attr.key.local_name().as_ref() {
                            b"name" => {
                                param_name = Some(String::from_utf8_lossy(&attr.value).to_string())
                            }
                            b"value" => {
                                param_value = Some(String::from_utf8_lossy(&attr.value).to_string())
                            }
                            _ => {}
                        }
                    }
                    if param_name.as_deref() == Some("accessURL") {
                        if let Some(value) = param_value.filter(|v| !v.trim().is_empty()) {
                            return Some(value);
                        }
                    }
                }
            }
            Ok(quick_xml::events::Event::Empty(e)) => {
                if in_target_resource && e.name().local_name().as_ref() == b"PARAM" {
                    let mut param_name = None;
                    let mut param_value = None;
                    for attr in e.attributes().flatten() {
                        match attr.key.local_name().as_ref() {
                            b"name" => {
                                param_name = Some(String::from_utf8_lossy(&attr.value).to_string())
                            }
                            b"value" => {
                                param_value = Some(String::from_utf8_lossy(&attr.value).to_string())
                            }
                            _ => {}
                        }
                    }
                    if param_name.as_deref() == Some("accessURL") {
                        if let Some(value) = param_value.filter(|v| !v.trim().is_empty()) {
                            return Some(value);
                        }
                    }
                }
            }
            Ok(quick_xml::events::Event::End(e))
                if e.name().local_name().as_ref() == b"RESOURCE" =>
            {
                in_target_resource = false;
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    None
}

fn extract_table_rows(fields: &[String], xml: &str) -> Vec<HashMap<String, String>> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut rows = Vec::new();
    let mut in_tr = false;
    let mut in_td = false;
    let mut cells = Vec::new();
    let mut current_cell = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e)) if e.name().local_name().as_ref() == b"TR" => {
                in_tr = true;
                cells.clear();
            }
            Ok(quick_xml::events::Event::End(e))
                if e.name().local_name().as_ref() == b"TR" && in_tr =>
            {
                let mut row = HashMap::new();
                for (i, field) in fields.iter().enumerate() {
                    row.insert(field.clone(), cells.get(i).cloned().unwrap_or_default());
                }
                rows.push(row);
                in_tr = false;
            }
            Ok(quick_xml::events::Event::Start(e))
                if e.name().local_name().as_ref() == b"TD" && in_tr =>
            {
                in_td = true;
                current_cell.clear();
            }
            Ok(quick_xml::events::Event::End(e))
                if e.name().local_name().as_ref() == b"TD" && in_tr =>
            {
                cells.push(std::mem::take(&mut current_cell));
                in_td = false;
            }
            Ok(quick_xml::events::Event::Empty(e))
                if in_tr && e.name().local_name().as_ref() == b"TD" =>
            {
                cells.push(String::new());
            }
            Ok(quick_xml::events::Event::Text(e)) if in_td => {
                if let Ok(text) = e.unescape() {
                    current_cell.push_str(&text);
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_casda_datalink_fixture() {
        let xml = include_str!("../tests/fixtures/casda_datalink.xml");
        let (soda_url, token) = parse_casda_datalink(xml, "async_service").unwrap();
        assert_eq!(
            soda_url,
            "https://casda.csiro.au/casda_data_access/data/async"
        );
        assert_eq!(token, "cube-244");
    }
}
