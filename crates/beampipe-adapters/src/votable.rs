use crate::{AdapterError, TapRow};
use quick_xml::events::Event;
use quick_xml::Reader;
use serde_json::{Map, Value};

/// Parse a minimal VOTable TABLE/DATA/TABLEDATA response into row maps.
pub fn parse_votable_xml(xml: &str) -> Result<Vec<TapRow>, AdapterError> {
    let fields = extract_field_names(xml);
    if fields.is_empty() {
        return Ok(Vec::new());
    }
    extract_table_rows(xml, &fields)
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
}
