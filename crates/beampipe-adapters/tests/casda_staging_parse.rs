use beampipe_adapters::parse_job_results;

#[test]
fn parse_visibility_and_checksum_by_scan_id() {
    let xml = r#"<?xml version="1.0"?>
    <uws:results xmlns:uws="http://www.ivoa.net/xml/UWS/v1.0">
      <uws:result id="visibility-105174"><uws:reference>https://example/a</uws:reference></uws:result>
      <uws:result id="visibility-105366"><uws:reference>https://example/b</uws:reference></uws:result>
      <uws:result id="visibility-105174.checksum"><uws:reference>https://example/cs</uws:reference></uws:result>
    </uws:results>"#;
    let (data, checksum) = parse_job_results(xml);
    assert_ne!(data.get("105174").unwrap(), data.get("105366").unwrap());
    assert_eq!(checksum.get("105174").unwrap(), "https://example/cs");
}
