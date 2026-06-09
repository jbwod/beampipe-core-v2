//! Startup security checks (JWT, Slurm SSH, CASDA, database URL).

use crate::slurm_credentials::{beampipe_env, is_production_env, SlurmSshCredentials};
use beampipe_config::Settings;
use beampipe_security::{bool_env, resolve_secret, SecretPolicy, SecretRef};

const DEV_JWT_SECRETS: &[&str] = &["secret-key", "local-dev-jwt-secret-change-me"];

fn security_strict_enabled() -> bool {
    if let Some(v) = bool_env("BEAMPIPE_SECURITY_STRICT") {
        return v;
    }
    is_production_env()
}

fn use_real_backends() -> bool {
    bool_env("BEAMPIPE_USE_REAL_BACKENDS").unwrap_or(false)
}

/// Collect security issues (always runs all checks; used by `beampipe security check`).
pub fn collect_security_issues(settings: &Settings) -> Vec<String> {
    let mut errors = Vec::new();

    if settings.jwt_secret.len() < 32 {
        errors.push(format!(
            "BEAMPIPE_JWT_SECRET must be at least 32 characters in {} (got {})",
            beampipe_env(),
            settings.jwt_secret.len()
        ));
    }
    if DEV_JWT_SECRETS.contains(&settings.jwt_secret.as_str()) {
        errors.push(
            "BEAMPIPE_JWT_SECRET is a known development default; set a unique secret for production"
                .into(),
        );
    }

    if settings.database_url.contains("postgres:postgres@")
        || settings.database_url.contains(":postgres@postgres")
    {
        errors.push(
            "DATABASE_URL appears to use the default postgres password; use a strong password in production"
                .into(),
        );
    }

    if is_production_env() {
        if settings.metrics_public {
            errors.push("BEAMPIPE_METRICS_PUBLIC=true is not allowed in production".into());
        }
        if settings.cors_allow_origins.is_none() {
            errors.push(
                "BEAMPIPE_CORS_ALLOW_ORIGINS must be set explicitly in production; permissive CORS is not allowed"
                    .into(),
            );
        }
        if settings.require_rate_limiter && settings.redis_url.is_none() {
            errors.push(
                "BEAMPIPE_REQUIRE_RATE_LIMITER=true requires BEAMPIPE_REDIS_URL in production"
                    .into(),
            );
        }
    }

    if use_real_backends() {
        match SlurmSshCredentials::resolve() {
            Ok(creds) => {
                if is_production_env() && creds.key_source.source_kind() == "env" {
                    errors.push(
                        "SLURM_SSH_PRIVATE_KEY inline PEM is not allowed in production without BEAMPIPE_ALLOW_INLINE_SECRETS=true"
                            .into(),
                    );
                }
                if is_production_env()
                    && !creds.strict_known_hosts
                    && !bool_env("BEAMPIPE_ALLOW_INSECURE_SSH_HOST_KEYS").unwrap_or(false)
                {
                    errors.push(
                        "Slurm strict known-hosts verification is required in production".into(),
                    );
                }
                if creds.strict_known_hosts {
                    match creds.known_hosts_path.as_ref() {
                        Some(path) if !std::path::Path::new(path).is_file() => {
                            errors.push(format!("SLURM_SSH_KNOWN_HOSTS file not found: {path}"));
                        }
                        Some(path) => {
                            if let Err(e) = crate::slurm_ssh::load_known_host_entries(path) {
                                errors.push(format!("SLURM_SSH_KNOWN_HOSTS invalid: {e}"));
                            }
                        }
                        None if is_production_env() => {
                            errors.push("SLURM_SSH_KNOWN_HOSTS is required in production".into());
                        }
                        None => {}
                    }
                }
            }
            Err(e) => errors.push(format!("Slurm SSH credentials: {e}")),
        }

        let casda_user = std::env::var("CASDA_USERNAME")
            .ok()
            .filter(|s| !s.is_empty());
        let casda_pass_ok = if let Ok(path) = std::env::var("CASDA_PASSWORD_FILE") {
            resolve_secret(
                &SecretRef::File { file: path },
                SecretPolicy::from_process_env(),
            )
            .is_ok()
        } else {
            std::env::var("CASDA_PASSWORD")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .is_some()
        };
        if casda_user.is_none() || !casda_pass_ok {
            errors.push(
                "CASDA_USERNAME and CASDA_PASSWORD or CASDA_PASSWORD_FILE are required when BEAMPIPE_USE_REAL_BACKENDS=true (staging)"
                    .into(),
            );
        }
    }

    errors
}

pub fn validate_security(settings: &Settings) -> Result<(), Vec<String>> {
    if !security_strict_enabled() {
        return Ok(());
    }
    let errors = collect_security_issues(settings);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
