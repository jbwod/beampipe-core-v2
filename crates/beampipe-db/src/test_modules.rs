//! Ephemeral `project_module` values used by integration tests against a real `DATABASE_URL`.

/// PromQL `project_module!~` filter for integration-test modules (keep in sync with [`is_integration_test_project_module`]).
pub const INTEGRATION_TEST_MODULE_REGEX: &str = "fail_requeue_.*|sig_test_.*|^test_.*|exec_sig_.*";

pub fn is_integration_test_project_module(module: &str) -> bool {
    module.starts_with("fail_requeue_")
        || module.starts_with("sig_test_")
        || module.starts_with("test_")
        || module.starts_with("exec_sig_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_integration_test_modules() {
        assert!(is_integration_test_project_module(
            "sig_test_019e80f3-1089-78b3-94d7-e22800f7a751"
        ));
        assert!(is_integration_test_project_module("exec_sig_abc"));
        assert!(!is_integration_test_project_module("wallaby_hires"));
    }
}
