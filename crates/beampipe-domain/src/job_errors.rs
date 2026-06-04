/// Whether a job failure should skip retry backoff (preflight, auth, unreachable services).
pub fn is_non_retryable_job_error(kind: &str, error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    if kind == "execute" {
        return lower.contains("unreachable")
            || lower.contains("authentication failed")
            || lower.contains("credentials required")
            || lower.contains("translation manager")
            || lower.contains("preflight")
            || lower.contains("dim at")
            || lower.contains("is unreachable");
    }
    if kind == "discover_batch" {
        return lower.contains("authentication failed") || lower.contains("401");
    }
    if kind == "slurm_poll_tick" || kind == "dim_poll" || kind == "dim_poll_tick" {
        return lower.contains("authentication failed")
            || lower.contains("connection failed")
            || lower.contains("unreachable");
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tm_unreachable_is_non_retryable() {
        assert!(is_non_retryable_job_error(
            "execute",
            "DALiuGE Translation Manager at http://dlg-tm.desk is unreachable"
        ));
    }

    #[test]
    fn casda_auth_is_non_retryable() {
        assert!(is_non_retryable_job_error(
            "execute",
            "CASDA authentication failed: HTTP 401"
        ));
    }

    #[test]
    fn transient_errors_retry() {
        assert!(!is_non_retryable_job_error("execute", "database timeout"));
    }
}
