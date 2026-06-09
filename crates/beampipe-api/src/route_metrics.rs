use uuid::Uuid;

/// Collapse UUID path segments for Prometheus labels.
pub fn normalize_api_route(path: &str) -> String {
    if path.is_empty() {
        return "/".to_string();
    }
    path.split('/')
        .map(|segment| {
            if segment.is_empty() {
                String::new()
            } else if Uuid::parse_str(segment).is_ok() {
                ":id".to_string()
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_segments_normalized() {
        let id = "019e80f2-cd6f-7883-93e1-62b9ccd35cdf";
        let path = format!("/api/v2/executions/{id}/status");
        assert_eq!(normalize_api_route(&path), "/api/v2/executions/:id/status");
    }
}
