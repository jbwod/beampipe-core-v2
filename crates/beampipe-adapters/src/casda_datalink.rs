use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct DataLinkDocument {
    #[serde(default, rename = "RESOURCE")]
    resources: Vec<DataLinkResource>,
}

#[derive(Debug, Deserialize)]
struct DataLinkResource {
    #[serde(default, rename = "@ID")]
    id: Option<String>,
    #[serde(default, rename = "PARAM")]
    parameters: Vec<DataLinkParameter>,
}

#[derive(Debug, Deserialize)]
struct DataLinkParameter {
    #[serde(default, rename = "@name")]
    name: String,
    #[serde(default, rename = "@value")]
    value: Option<String>,
}

/// Parse a CASDA DataLink VOTable and return the SODA async URL plus ID token for
/// `service_name`.
pub fn parse_casda_datalink(xml: &str, service_name: &str) -> Option<(String, String)> {
    let soda_url = extract_soda_access_url(xml, service_name)?;
    for row in crate::votable::parse_votable_xml(xml).ok()? {
        let service = row.get("service_def").and_then(Value::as_str).unwrap_or("");
        if service != service_name {
            continue;
        }
        let token = row
            .get("authenticated_id_token")
            .and_then(Value::as_str)
            .filter(|token| !token.trim().is_empty());
        if let Some(token) = token {
            return Some((soda_url, token.to_owned()));
        }
    }
    None
}

// Real CASDA DataLink documents contain input PARAM elements without `value`, which the full
// standards model correctly rejects. A deliberately narrow typed projection keeps those optional
// while all FIELD and TABLEDATA parsing remains delegated to the `votable` crate above.
fn extract_soda_access_url(xml: &str, service_name: &str) -> Option<String> {
    let document = quick_xml::de::from_str::<DataLinkDocument>(xml).ok()?;
    document
        .resources
        .into_iter()
        .find(|resource| resource.id.as_deref() == Some(service_name))?
        .parameters
        .into_iter()
        .find(|parameter| parameter.name == "accessURL")?
        .value
        .filter(|url| !url.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_casda_datalink_golden() {
        let xml = include_str!("../tests/fixtures/casda_datalink.xml");
        let (soda_url, token) = parse_casda_datalink(xml, "async_service").unwrap();
        assert_eq!(
            soda_url,
            "https://casda.csiro.au/casda_data_access/data/async"
        );
        assert_eq!(token, "cube-244");
    }

    #[test]
    fn selects_only_an_authenticated_row_for_the_requested_service() {
        let xml = r#"<?xml version="1.0"?>
<v:VOTABLE xmlns:v="http://www.ivoa.net/xml/VOTable/v1.4" version="1.4">
  <v:RESOURCE type="results"><v:TABLE>
    <v:FIELD datatype="char" arraysize="*" name="service_def"/>
    <v:FIELD datatype="char" arraysize="*" name="authenticated_id_token"/>
    <v:DATA><v:TABLEDATA>
      <v:TR><v:TD>other_service</v:TD><v:TD>wrong-service</v:TD></v:TR>
      <v:TR><v:TD>async_service</v:TD><v:TD/></v:TR>
      <v:TR><v:TD>async_service</v:TD><v:TD>right-token</v:TD></v:TR>
    </v:TABLEDATA></v:DATA>
  </v:TABLE></v:RESOURCE>
  <v:RESOURCE ID="other_service" type="meta">
    <v:PARAM name="accessURL" datatype="char" arraysize="*" value="https://example.test/wrong"/>
  </v:RESOURCE>
  <v:RESOURCE ID="async_service" type="meta">
    <v:PARAM name="accessURL" datatype="char" arraysize="*" value="https://example.test/right"/>
  </v:RESOURCE>
</v:VOTABLE>"#;

        assert_eq!(
            parse_casda_datalink(xml, "async_service"),
            Some((
                "https://example.test/right".to_owned(),
                "right-token".to_owned()
            ))
        );
    }
}
