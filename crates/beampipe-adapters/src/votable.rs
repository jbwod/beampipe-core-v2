use crate::{AdapterError, TapRow};
use quick_xml::events::Event;
use quick_xml::Reader;
use serde_json::{Map, Value};

/// Parse a minimal VOTable TABLE/DATA/TABLEDATA response into row maps.
pub fn parse_votable_xml(xml: &str) -> Result<Vec<TapRow>, AdapterError> {
    reject_tap_query_error(xml)?;
    let fields = extract_field_names(xml);
    if fields.is_empty() {
        return Ok(Vec::new());
    }
    extract_table_rows(xml, &fields)
}

fn reject_tap_query_error(xml: &str) -> Result<(), AdapterError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut error_message: Option<String> = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if element_name_is(e.name().as_ref(), b"INFO") => {
                if is_tap_query_error(&e) {
                    error_message = Some(String::new());
                }
            }
            Ok(Event::Empty(e)) if element_name_is(e.name().as_ref(), b"INFO") => {
                if is_tap_query_error(&e) {
                    return Err(tap_query_error(None));
                }
            }
            Ok(Event::Text(e)) if error_message.is_some() => {
                if let Ok(text) = e.unescape() {
                    error_message.as_mut().unwrap().push_str(&text);
                }
            }
            Ok(Event::CData(e)) if error_message.is_some() => {
                error_message
                    .as_mut()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(e.as_ref()));
            }
            Ok(Event::End(e))
                if error_message.is_some() && element_name_is(e.name().as_ref(), b"INFO") =>
            {
                return Err(tap_query_error(error_message.as_deref()));
            }
            Ok(Event::Eof) => {
                return match error_message.as_deref() {
                    Some(message) => Err(tap_query_error(Some(message))),
                    None => Ok(()),
                };
            }
            Err(error) => {
                return Err(AdapterError::InvalidRowShape(format!(
                    "VOTable status parse error: {error}"
                )));
            }
            _ => {}
        }
        buf.clear();
    }
}

fn element_name_is(actual: &[u8], expected: &[u8]) -> bool {
    actual
        .rsplit(|byte| *byte == b':')
        .next()
        .unwrap_or(actual)
        .eq_ignore_ascii_case(expected)
}

fn attr_value_ignore_ascii_case(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .filter_map(|attribute| attribute.ok())
        .find(|attribute| attribute.key.as_ref().eq_ignore_ascii_case(key))
        .and_then(|attribute| String::from_utf8(attribute.value.into_owned()).ok())
}

fn is_tap_query_error(e: &quick_xml::events::BytesStart) -> bool {
    attr_value_ignore_ascii_case(e, b"name")
        .is_some_and(|name| name.eq_ignore_ascii_case("QUERY_STATUS"))
        && attr_value_ignore_ascii_case(e, b"value")
            .is_some_and(|value| value.eq_ignore_ascii_case("ERROR"))
}

fn tap_query_error(message: Option<&str>) -> AdapterError {
    let compact = message
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    const MAX_MESSAGE_CHARS: usize = 240;
    let message = if compact.is_empty() {
        "TAP service reported QUERY_STATUS=ERROR".to_string()
    } else if compact.chars().count() <= MAX_MESSAGE_CHARS {
        compact
    } else {
        format!(
            "{}...",
            compact.chars().take(MAX_MESSAGE_CHARS).collect::<String>()
        )
    };
    AdapterError::Permanent(message)
}

fn attr_value(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .filter_map(|a| a.ok())
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| String::from_utf8(a.value.into_owned()).ok())
}

fn extract_field_names(xml: &str) -> Vec<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut names = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) if e.name().as_ref() == b"FIELD" => {
                if let Some(name) = attr_value(&e, b"name").or_else(|| attr_value(&e, b"ID")) {
                    names.push(name);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                tracing::warn!(error = %e, "event=votable_field_parse_error");
                break;
            }
            _ => {}
        }
        buf.clear();
    }
    names
}

fn extract_table_rows(xml: &str, fields: &[String]) -> Result<Vec<TapRow>, AdapterError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut rows = Vec::new();
    let mut in_tr = false;
    let mut in_td = false;
    let mut cells = Vec::new();
    let mut current_cell = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.name().as_ref() == b"TR" => {
                in_tr = true;
                cells.clear();
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"TR" && in_tr => {
                let mut row = Map::new();
                for (i, field) in fields.iter().enumerate() {
                    row.insert(
                        field.clone(),
                        Value::String(cells.get(i).cloned().unwrap_or_default()),
                    );
                }
                rows.push(row);
                in_tr = false;
            }
            Ok(Event::Start(e)) if e.name().as_ref() == b"TD" && in_tr => {
                in_td = true;
                current_cell.clear();
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"TD" && in_tr => {
                cells.push(std::mem::take(&mut current_cell));
                in_td = false;
            }
            Ok(Event::Empty(e)) if in_tr && e.name().as_ref() == b"TD" => {
                cells.push(String::new());
            }
            Ok(Event::Text(e)) if in_td => {
                if let Ok(text) = e.unescape() {
                    current_cell.push_str(&text);
                }
            }
            Ok(Event::CData(e)) if in_td => {
                current_cell.push_str(&String::from_utf8_lossy(e.as_ref()));
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(AdapterError::InvalidRowShape(format!(
                    "VOTable row parse error: {e}"
                )));
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_casda_style_votable_with_non_empty_fields() {
        let xml = r#"<?xml version="1.0"?><VOTABLE><RESOURCE><TABLE>
<FIELD name="obs_id"/><FIELD name="s_ra"/><FIELD name="s_dec"/>
<DATA><TABLEDATA><TR><TD>ASKAP-72962</TD><TD>198.39</TD><TD>-15.45</TD></TR></TABLEDATA></DATA>
</TABLE></RESOURCE></VOTABLE>"#;
        let rows = parse_votable_xml(xml).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["obs_id"], "ASKAP-72962");
        assert_eq!(rows[0]["s_ra"], "198.39");
        assert_eq!(rows[0]["s_dec"], "-15.45");
    }

    #[test]
    fn empty_td_cells_preserve_column_alignment() {
        let xml = r#"<?xml version="1.0"?><VOTABLE><RESOURCE><TABLE>
<FIELD name="a"/><FIELD name="b"/>
<DATA><TABLEDATA><TR><TD>1</TD><TD></TD></TR></TABLEDATA></DATA>
</TABLE></RESOURCE></VOTABLE>"#;
        let rows = parse_votable_xml(xml).unwrap();
        assert_eq!(rows[0]["a"], "1");
        assert_eq!(rows[0]["b"], "");
    }

    #[test]
    fn rejects_tap_query_status_error_with_bounded_text() {
        let long_detail = "invalid query ".repeat(40);
        let xml = format!(
            r#"<?xml version="1.0"?><VOTABLE><RESOURCE><INFO name="QUERY_STATUS" value="ERROR">{long_detail}</INFO></RESOURCE></VOTABLE>"#
        );

        let error = parse_votable_xml(&xml).unwrap_err();
        match error {
            AdapterError::Permanent(message) => {
                assert!(message.starts_with("invalid query"));
                assert!(message.chars().count() <= 243);
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn rejects_empty_tap_query_status_error_info() {
        let xml = r#"<?xml version="1.0"?><VOTABLE><RESOURCE><INFO name="QUERY_STATUS" value="ERROR"/></RESOURCE></VOTABLE>"#;

        let error = parse_votable_xml(xml).unwrap_err();
        assert!(matches!(&error, AdapterError::Permanent(_)));
        assert!(error.to_string().contains("QUERY_STATUS=ERROR"));
    }
}
