use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;
use std::path::PathBuf;
use thiserror::Error;
use utoipa::ToSchema;
use zeroize::Zeroizing;

pub const REDACTED: &str = "[REDACTED]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretPolicy {
    Development,
    Production,
}

impl SecretPolicy {
    pub fn from_env_name(env: &str) -> Self {
        if is_production_env_name(env) {
            Self::Production
        } else {
            Self::Development
        }
    }

    pub fn from_process_env() -> Self {
        Self::from_env_name(&std::env::var("BEAMPIPE_ENV").unwrap_or_else(|_| "development".into()))
    }

    pub fn allow_inline(self) -> bool {
        matches!(self, Self::Development) || allow_inline_secrets_override()
    }
}

pub fn allow_inline_secrets_override() -> bool {
    bool_env("BEAMPIPE_ALLOW_INLINE_SECRETS").unwrap_or(false)
}

pub fn normalize_env_name(env: &str) -> String {
    env.trim().to_ascii_lowercase()
}

pub fn is_production_env_name(env: &str) -> bool {
    matches!(normalize_env_name(env).as_str(), "production" | "prod")
}

pub fn process_env_name() -> String {
    std::env::var("BEAMPIPE_ENV").unwrap_or_else(|_| "development".into())
}

pub fn is_process_production() -> bool {
    is_production_env_name(&process_env_name())
}

pub fn parse_bool_value(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "y" | "on" => Some(true),
        "0" | "false" | "no" | "n" | "off" => Some(false),
        _ => None,
    }
}

pub fn bool_env(name: &str) -> Option<bool> {
    std::env::var(name).ok().and_then(|v| parse_bool_value(&v))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(untagged)]
pub enum SecretRef {
    Env { env: String },
    File { file: String },
    InlineDev { inline_dev: String },
}

impl SecretRef {
    pub fn source_kind(&self) -> &'static str {
        match self {
            Self::Env { .. } => "env",
            Self::File { .. } => "file",
            Self::InlineDev { .. } => "inline_dev",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(Zeroizing<String>);

impl SecretValue {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue([REDACTED])")
    }
}

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("secret env {0} is not set")]
    MissingEnv(String),
    #[error("secret env {0} is empty")]
    EmptyEnv(String),
    #[error("secret file {0} is empty")]
    EmptyFile(String),
    #[error("read secret file {path}: {source}")]
    ReadFile {
        path: String,
        source: std::io::Error,
    },
    #[error("inline secrets are not allowed in production")]
    InlineNotAllowed,
}

pub fn resolve_secret(
    reference: &SecretRef,
    policy: SecretPolicy,
) -> Result<SecretValue, SecretError> {
    let value = match reference {
        SecretRef::Env { env } => {
            let value = std::env::var(env).map_err(|_| SecretError::MissingEnv(env.clone()))?;
            if value.trim().is_empty() {
                return Err(SecretError::EmptyEnv(env.clone()));
            }
            value
        }
        SecretRef::File { file } => {
            let path = PathBuf::from(file);
            let value = std::fs::read_to_string(&path).map_err(|source| SecretError::ReadFile {
                path: file.clone(),
                source,
            })?;
            let trimmed = value.trim_end_matches(['\r', '\n']).to_string();
            if trimmed.is_empty() {
                return Err(SecretError::EmptyFile(file.clone()));
            }
            trimmed
        }
        SecretRef::InlineDev { inline_dev } => {
            if !policy.allow_inline() {
                return Err(SecretError::InlineNotAllowed);
            }
            if inline_dev.trim().is_empty() {
                return Err(SecretError::EmptyEnv("inline_dev".into()));
            }
            inline_dev.clone()
        }
    };
    Ok(SecretValue(Zeroizing::new(value)))
}

pub fn parse_secret_ref(value: &Value) -> Option<SecretRef> {
    serde_json::from_value(value.clone()).ok()
}

pub fn resolve_secret_value(
    value: &Value,
    policy: SecretPolicy,
) -> Result<Option<SecretValue>, SecretError> {
    match parse_secret_ref(value) {
        Some(reference) => resolve_secret(&reference, policy).map(Some),
        None => Ok(value
            .as_str()
            .map(|s| SecretValue(Zeroizing::new(s.to_string())))),
    }
}

pub fn resolve_secret_value_strict(
    value: &Value,
    policy: SecretPolicy,
) -> Result<Option<SecretValue>, SecretError> {
    match parse_secret_ref(value) {
        Some(reference) => resolve_secret(&reference, policy).map(Some),
        None if value.is_null() => Ok(None),
        None if value.as_str().is_some() && policy.allow_inline() => Ok(value
            .as_str()
            .map(|s| SecretValue(Zeroizing::new(s.to_string())))),
        None if value.as_str().is_some() => Err(SecretError::InlineNotAllowed),
        None => Ok(None),
    }
}

pub fn is_secret_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    [
        "password",
        "passphrase",
        "passcode",
        "token",
        "secret",
        "credential",
        "authorization",
        "routing_key",
        "private_key",
        "webhook",
        "api_key",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub fn redact_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, val) in map {
                if is_secret_key(key) {
                    out.insert(key.clone(), Value::String(REDACTED.into()));
                } else {
                    out.insert(key.clone(), redact_value(val));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(redact_value).collect()),
        Value::String(s) => Value::String(redact_string(s)),
        _ => value.clone(),
    }
}

pub fn redact_string(input: &str) -> String {
    let mut out = input.to_string();
    for marker in [
        "password=",
        "passphrase=",
        "passcode=",
        "token=",
        "access_token=",
        "id_token=",
        "secret=",
        "apikey=",
        "api_key=",
        "authorization=",
    ] {
        out = redact_after_marker(&out, marker);
    }
    out
}

fn redact_after_marker(input: &str, marker: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let mut result = String::new();
    let mut cursor = 0;
    let mut search_from = 0;
    while let Some(pos) = lower[search_from..].find(marker) {
        let start = search_from + pos;
        let value_start = start + marker.len();
        result.push_str(&input[cursor..value_start]);
        result.push_str(REDACTED);
        let end = input[value_start..]
            .find(|c: char| c == '&' || c.is_whitespace() || c == '"' || c == '\'')
            .map(|n| value_start + n)
            .unwrap_or(input.len());
        cursor = end;
        search_from = end;
    }
    result.push_str(&input[cursor..]);
    result
}

pub fn secret_paths(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    collect_secret_paths(value, "", &mut out);
    out
}

fn collect_secret_paths(value: &Value, prefix: &str, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, val) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                if is_secret_key(key) || parse_secret_ref(val).is_some() {
                    out.push(path.clone());
                }
                collect_secret_paths(val, &path, out);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_secret_paths(item, &format!("{prefix}[{index}]"), out);
            }
        }
        _ => {}
    }
}

pub fn unsafe_inline_secret_paths(value: &Value, policy: SecretPolicy) -> Vec<String> {
    if policy.allow_inline() {
        return Vec::new();
    }
    let mut out = Vec::new();
    collect_unsafe_inline(value, "", &mut out);
    out
}

fn collect_unsafe_inline(value: &Value, prefix: &str, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if matches!(parse_secret_ref(value), Some(SecretRef::InlineDev { .. })) {
                out.push(prefix.to_string());
                return;
            }
            for (key, val) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                if is_secret_key(key) && val.as_str().is_some() {
                    out.push(path.clone());
                }
                collect_unsafe_inline(val, &path, out);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_unsafe_inline(item, &format!("{prefix}[{index}]"), out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn production_rejects_inline_dev() {
        let reference = SecretRef::InlineDev {
            inline_dev: "secret".into(),
        };
        assert!(matches!(
            resolve_secret(&reference, SecretPolicy::Production),
            Err(SecretError::InlineNotAllowed)
        ));
    }

    #[test]
    fn production_env_name_is_normalized() {
        assert_eq!(
            SecretPolicy::from_env_name(" Production "),
            SecretPolicy::Production
        );
        assert_eq!(
            SecretPolicy::from_env_name("prod"),
            SecretPolicy::Production
        );
        assert_eq!(
            SecretPolicy::from_env_name("development"),
            SecretPolicy::Development
        );
    }

    #[test]
    fn bool_values_are_normalized() {
        assert_eq!(parse_bool_value(" TRUE "), Some(true));
        assert_eq!(parse_bool_value("off"), Some(false));
        assert_eq!(parse_bool_value("maybe"), None);
    }

    #[test]
    fn strict_secret_value_rejects_raw_string_in_production() {
        let value = json!("literal-secret");
        assert!(matches!(
            resolve_secret_value_strict(&value, SecretPolicy::Production),
            Err(SecretError::InlineNotAllowed)
        ));
    }

    #[test]
    fn redacts_nested_secret_keys_and_url_markers() {
        let value = json!({
            "headers": {"Authorization": "Bearer abc"},
            "url": "https://x.test/path?access_token=abc&ok=1",
            "nested": [{"smtp_password": "pw"}]
        });
        let redacted = redact_value(&value);
        assert_eq!(redacted["headers"]["Authorization"], REDACTED);
        assert_eq!(redacted["nested"][0]["smtp_password"], REDACTED);
        assert!(redacted["url"].as_str().unwrap().contains(REDACTED));
    }

    #[test]
    fn finds_unsafe_inline_secret_paths() {
        let value = json!({
            "password": "pw",
            "ok": {"env": "OK"},
            "token_ref": {"inline_dev": "local"}
        });
        let paths = unsafe_inline_secret_paths(&value, SecretPolicy::Production);
        assert!(paths.contains(&"password".to_string()));
        assert!(paths.contains(&"token_ref".to_string()));
    }
}
