use beampipe_adapters::{parse_eval_job_results, parse_job_results};

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

#[test]
fn parse_casda_xlink_result_attributes() {
    let xml = r#"<?xml version="1.0"?>
    <uws:results xmlns:uws="http://www.ivoa.net/xml/UWS/v1.0"
                 xmlns:xlink="http://www.w3.org/1999/xlink">
      <uws:result id="evaluation-90293" xlink:href="https://example.test/cache/calibration-metadata-processing-logs-SB72962_2025-04-21-063210.tar?token=redacted"/>
      <uws:result id="evaluation-90293.checksum" xlink:href="https://example.test/cache/calibration-metadata-processing-logs-SB72962_2025-04-21-063210.tar.checksum?token=redacted"/>
    </uws:results>"#;
    let filename = "calibration-metadata-processing-logs-SB72962_2025-04-21-063210.tar";
    let (data, checksum) = parse_eval_job_results(xml);
    assert!(data.contains_key(filename));
    assert!(checksum.contains_key(filename));
}
