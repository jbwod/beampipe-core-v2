use crate::{AdapterError, TapRow};
use quick_xml::events::Event;
use quick_xml::Reader;
use serde_json::{Map, Value};
use votable::data::TableOrBinOrBin2;
use votable::iter::strings::RowStringIterator;
use votable::iter::SimpleVOTableRowIterator;
use votable::table::TableElem;

/// Parse the first result table in a VOTable response into TAP row maps.
pub fn parse_votable_xml(xml: &str) -> Result<Vec<TapRow>, AdapterError> {
    // TAP errors are frequently valid VOTables with no TABLE. Check the status first so
    // callers retain the service-provided diagnostic instead of a generic shape error.
    reject_tap_query_error(xml)?;
    let mut table_rows =
        SimpleVOTableRowIterator::from_reader(xml.as_bytes()).map_err(votable_shape_error)?;
    if !matches!(table_rows.data_type(), TableOrBinOrBin2::TableData) {
        return Err(AdapterError::InvalidRowShape(
            "VOTable result does not use TABLEDATA".into(),
        ));
    }
    let fields = table_rows
        .votable()
        .get_first_table()
        .into_iter()
        .flat_map(|table| table.elems.iter())
        .filter_map(|element| match element {
            TableElem::Field(field) => Some(field.name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if fields.is_empty() {
        return Ok(Vec::new());
    }
    let rows = {
        let (reader, buffer) = table_rows.borrow_mut_reader_and_buff();
        RowStringIterator::new(reader, buffer)
            .collect::<Result<Vec<_>, _>>()
            .map_err(votable_shape_error)?
    };

    Ok(rows
        .iter()
        .map(|cells| {
            let mut row = Map::new();
            for (index, field) in fields.iter().enumerate() {
                row.insert(
                    field.clone(),
                    Value::String(cells.get(index).cloned().unwrap_or_default()),
                );
            }
            row
        })
        .collect())
}

fn votable_shape_error(error: votable::VOTableError) -> AdapterError {
    AdapterError::InvalidRowShape(format!("VOTable parse error: {error}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_namespaced_tabledata_golden() {
        let xml = include_str!("../tests/fixtures/tap_namespaced_tabledata.xml");
        let rows = parse_votable_xml(xml).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["obs_id"], "ASKAP-72962");
        assert_eq!(rows[0]["s_ra"], "198.39");
        assert_eq!(rows[0]["s_dec"], "-15.45");
        assert_eq!(rows[1]["obs_id"], "ASKAP-72963 & follow-up");
        assert_eq!(rows[1]["s_ra"], "");
        assert_eq!(rows[1]["s_dec"], "-16.00");
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
    fn rejects_empty_namespaced_tap_query_status_error_info() {
        let xml = r#"<?xml version="1.0"?><v:VOTABLE xmlns:v="http://www.ivoa.net/xml/VOTable/v1.4"><v:RESOURCE><v:INFO name="QUERY_STATUS" value="ERROR"/></v:RESOURCE></v:VOTABLE>"#;

        let error = parse_votable_xml(xml).unwrap_err();
        assert!(matches!(&error, AdapterError::Permanent(_)));
        assert!(error.to_string().contains("QUERY_STATUS=ERROR"));
    }

    #[test]
    fn reports_invalid_votable_shape_without_panicking() {
        let xml = r#"<VOTABLE version="1.4"><RESOURCE><TABLE><FIELD name="missing_datatype"/></TABLE></RESOURCE></VOTABLE>"#;
        let error = parse_votable_xml(xml).unwrap_err();
        assert!(matches!(error, AdapterError::InvalidRowShape(_)));
    }
}
