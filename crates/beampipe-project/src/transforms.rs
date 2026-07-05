use crate::{
    MappingSpec, ProjectConfig, TemplateVarSpec, TransformKind, TransformRef, TransformSpec,
    ValidationDiagnostic,
};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct TransformRegistry {
    named: BTreeMap<String, TransformSpec>,
}

impl TransformRegistry {
    pub fn from_config(config: &ProjectConfig) -> Self {
        let mut named = BTreeMap::new();
        if let Some(defs) = &config.definitions {
            named.extend(defs.transforms.clone());
        }
        Self { named }
    }

    pub fn resolve_spec(&self, name: &str) -> Option<TransformSpec> {
        if let Some(spec) = self.named.get(name) {
            return Some(spec.clone());
        }
        None
    }

    pub fn resolve_spec_with_legacy(&self, name: &str) -> Option<TransformSpec> {
        self.resolve_spec(name)
            .or_else(|| legacy_transform_spec(name))
    }

    pub fn apply_named(&self, name: &str, input: &Value) -> Option<Value> {
        let spec = self.resolve_spec(name)?;
        if spec.kind == TransformKind::Chain {
            return apply_chain(self, &spec, input);
        }
        apply_transform_spec(&spec, input)
    }

    pub fn apply_named_with_legacy(&self, name: &str, input: &Value) -> Option<Value> {
        let spec = self.resolve_spec_with_legacy(name)?;
        if spec.kind == TransformKind::Chain {
            return apply_chain(self, &spec, input);
        }
        apply_transform_spec(&spec, input)
    }

    pub fn apply_steps(&self, steps: &[String], input: &Value) -> Option<Value> {
        let mut current = input.clone();
        for step in steps {
            current = self.apply_named(step, &current)?;
        }
        Some(current)
    }
}

fn apply_chain(registry: &TransformRegistry, spec: &TransformSpec, input: &Value) -> Option<Value> {
    let steps = spec.steps.as_ref()?;
    registry.apply_steps(steps, input)
}

fn legacy_transform_spec(name: &str) -> Option<TransformSpec> {
    let mut spec = TransformSpec {
        kind: TransformKind::Unknown,
        prefix: None,
        suffix: None,
        separators: None,
        pattern: None,
        group: None,
        from: None,
        to: None,
        default: None,
        steps: None,
    };
    match name {
        "strip_hipass_prefix" => {
            spec.kind = TransformKind::StripPrefix;
            spec.prefix = Some("HIPASS".into());
        }
        "extract_askap_sbid" => spec.kind = TransformKind::ExtractDigits,
        "extract_scan_id" => {
            spec.kind = TransformKind::SplitLast;
            spec.separators = Some(vec!["/".into(), ":".into(), "#".into()]);
        }
        "is_present" => spec.kind = TransformKind::IsPresent,
        "select_eval_file_by_size" => spec.kind = TransformKind::SelectEvalFileBySize,
        "identity" => spec.kind = TransformKind::Identity,
        _ => return None,
    }
    Some(spec)
}

pub fn apply_transform_spec(spec: &TransformSpec, input: &Value) -> Option<Value> {
    match spec.kind {
        TransformKind::Identity => Some(input.clone()),
        TransformKind::Trim => {
            value_string(Some(input)).map(|s| Value::String(s.trim().to_string()))
        }
        TransformKind::Lowercase => {
            value_string(Some(input)).map(|s| Value::String(s.to_ascii_lowercase()))
        }
        TransformKind::Uppercase => {
            value_string(Some(input)).map(|s| Value::String(s.to_ascii_uppercase()))
        }
        TransformKind::Replace => {
            let raw = value_string(Some(input))?;
            let from = spec.from.as_deref()?;
            let to = spec.to.as_deref().unwrap_or("");
            Some(Value::String(raw.replace(from, to)))
        }
        TransformKind::AddPrefix => {
            let prefix = spec.prefix.as_deref()?;
            let raw = value_string(Some(input))?;
            Some(Value::String(format!("{prefix}{raw}")))
        }
        TransformKind::AddSuffix => {
            let suffix = spec.suffix.as_deref()?;
            let raw = value_string(Some(input))?;
            Some(Value::String(format!("{raw}{suffix}")))
        }
        TransformKind::DefaultIfEmpty => {
            if is_empty_value(input) {
                spec.default
                    .as_ref()
                    .map(|d| Value::String(d.clone()))
                    .or(Some(Value::Null))
            } else {
                Some(input.clone())
            }
        }
        TransformKind::StripPrefix => {
            let prefix = spec.prefix.as_deref()?;
            let raw = value_string(Some(input))?;
            Some(Value::String(
                raw.strip_prefix(prefix).unwrap_or(&raw).to_string(),
            ))
        }
        TransformKind::ExtractDigits => extract_digits(input).map(Value::String),
        TransformKind::SplitLast => {
            let raw = value_string(Some(input))?;
            let separators: Vec<char> = spec
                .separators
                .as_ref()
                .map(|items| items.iter().flat_map(|s| s.chars()).collect())
                .unwrap_or_else(|| vec!['/', ':', '#']);
            let segment = raw
                .rsplit(|c| separators.contains(&c))
                .next()
                .unwrap_or(raw.as_str())
                .trim();
            Some(Value::String(segment.to_string()))
        }
        TransformKind::IsPresent => Some(json!(!is_empty_value(input))),
        TransformKind::SelectEvalFileBySize => select_eval_file_by_size(input),
        TransformKind::RegexExtract => {
            let pattern = spec.pattern.as_deref()?;
            let group = spec.group.unwrap_or(1) as usize;
            let raw = value_string(Some(input))?;
            let re = regex::Regex::new(pattern).ok()?;
            let caps = re.captures(&raw)?;
            caps.get(group)
                .map(|m| Value::String(m.as_str().to_string()))
        }
        TransformKind::Chain | TransformKind::Unknown => None,
    }
}

pub fn build_template_context(
    source_identifier: &str,
    config: &ProjectConfig,
) -> Map<String, Value> {
    let registry = TransformRegistry::from_config(config);
    let mut context = Map::new();

    if let Some(_identity) = &config.source_identity {
        for (var_name, spec) in &_identity.template_vars {
            let base = template_var_base(source_identifier, spec);
            let value = if let Some(transform_name) = spec.transform.as_deref() {
                registry.apply_named(transform_name, &base).unwrap_or(base)
            } else {
                base
            };
            context.insert(var_name.clone(), value);
        }
        context
            .entry("source_identifier".to_string())
            .or_insert_with(|| json!(source_identifier));
        return context;
    }

    context.insert("source_identifier".into(), json!(source_identifier));
    let legacy_transform = config
        .discovery
        .queries
        .first()
        .and_then(|q| q.source_id_transform.as_deref());
    let source_name = legacy_transform
        .and_then(|name| registry.apply_named_with_legacy(name, &json!(source_identifier)))
        .and_then(|v| value_string(Some(&v)))
        .unwrap_or_else(|| source_identifier.to_string());
    context.insert("source_name".into(), json!(source_name));
    context
}

fn template_var_base(source_identifier: &str, spec: &TemplateVarSpec) -> Value {
    match spec.from.as_deref() {
        Some("canonical") | None => json!(source_identifier),
        Some(other) => json!(other),
    }
}

pub fn apply_field_transform(
    registry: &TransformRegistry,
    spec: &MappingSpec,
    input: &Value,
) -> Option<Value> {
    match spec.transform.as_ref()? {
        TransformRef::Name(name) => registry.apply_named(name, input),
        TransformRef::Chain(steps) => registry.apply_steps(steps, input),
    }
}

pub fn validate_transform_refs(config: &ProjectConfig) -> Vec<ValidationDiagnostic> {
    let registry = TransformRegistry::from_config(config);
    let mut errors = Vec::new();

    fn check(
        registry: &TransformRegistry,
        errors: &mut Vec<ValidationDiagnostic>,
        name: &str,
        location: &str,
    ) {
        if registry.resolve_spec(name).is_none() {
            errors.push(ValidationDiagnostic::error(
                location,
                "unknown_transform",
                format!("unknown transform '{name}'"),
            ));
        }
    }

    fn check_transform_ref(
        registry: &TransformRegistry,
        errors: &mut Vec<ValidationDiagnostic>,
        transform: &TransformRef,
        location: &str,
    ) {
        match transform {
            TransformRef::Name(name) => check(registry, errors, name, location),
            TransformRef::Chain(steps) => {
                if steps.is_empty() {
                    errors.push(ValidationDiagnostic::error(
                        location,
                        "empty_transform_chain",
                        "transform chain must include at least one step",
                    ));
                }
                for (i, step) in steps.iter().enumerate() {
                    check(registry, errors, step, &format!("{location}[{i}]"));
                }
            }
        }
    }

    if let Some(identity) = &config.source_identity {
        for (var_name, spec) in &identity.template_vars {
            if let Some(name) = spec.transform.as_deref() {
                check(
                    &registry,
                    &mut errors,
                    name,
                    &format!("source_identity.template_vars.{var_name}.transform"),
                );
            }
        }
    }

    for (i, query) in config.discovery.queries.iter().enumerate() {
        if let Some(name) = query.source_id_transform.as_deref() {
            check(
                &registry,
                &mut errors,
                name,
                &format!("discovery.queries[{i}].source_id_transform"),
            );
        }
    }

    if let Some(prepare) = &config.discovery.prepare_metadata {
        for (field, spec) in &prepare.field_map {
            if spec.from.trim().is_empty() {
                errors.push(ValidationDiagnostic::error(
                    format!("discovery.prepare_metadata.field_map.{field}.from"),
                    "required",
                    "field_map entries require from",
                ));
            }
            if let Some(transform) = spec.transform.as_ref() {
                check_transform_ref(
                    &registry,
                    &mut errors,
                    transform,
                    &format!("discovery.prepare_metadata.field_map.{field}.transform"),
                );
            }
        }
        for (flag, spec) in &prepare.discovery_flags {
            if spec.from.trim().is_empty() {
                errors.push(ValidationDiagnostic::error(
                    format!("discovery.prepare_metadata.discovery_flags.{flag}.from"),
                    "required",
                    "discovery_flags entries require from",
                ));
            }
            if let Some(transform) = spec.transform.as_ref() {
                check_transform_ref(
                    &registry,
                    &mut errors,
                    transform,
                    &format!("discovery.prepare_metadata.discovery_flags.{flag}.transform"),
                );
            }
        }
    }

    if let Some(defs) = &config.definitions {
        for (name, spec) in &defs.transforms {
            if spec.kind == TransformKind::Unknown {
                errors.push(ValidationDiagnostic::error(
                    format!("definitions.transforms.{name}.kind"),
                    "unknown_transform_kind",
                    "transform has an unknown kind",
                ));
            }
            if spec.kind == TransformKind::StripPrefix
                && spec.prefix.as_deref().unwrap_or("").is_empty()
            {
                errors.push(ValidationDiagnostic::error(
                    format!("definitions.transforms.{name}.prefix"),
                    "required",
                    "strip_prefix requires prefix",
                ));
            }
            if spec.kind == TransformKind::AddPrefix
                && spec.prefix.as_deref().unwrap_or("").is_empty()
            {
                errors.push(ValidationDiagnostic::error(
                    format!("definitions.transforms.{name}.prefix"),
                    "required",
                    "add_prefix requires prefix",
                ));
            }
            if spec.kind == TransformKind::AddSuffix
                && spec.suffix.as_deref().unwrap_or("").is_empty()
            {
                errors.push(ValidationDiagnostic::error(
                    format!("definitions.transforms.{name}.suffix"),
                    "required",
                    "add_suffix requires suffix",
                ));
            }
            if spec.kind == TransformKind::Replace && spec.from.as_deref().unwrap_or("").is_empty()
            {
                errors.push(ValidationDiagnostic::error(
                    format!("definitions.transforms.{name}.from"),
                    "required",
                    "replace requires from",
                ));
            }
            if spec.kind == TransformKind::RegexExtract
                && spec.pattern.as_deref().unwrap_or("").is_empty()
            {
                errors.push(ValidationDiagnostic::error(
                    format!("definitions.transforms.{name}.pattern"),
                    "required",
                    "regex_extract requires pattern",
                ));
            }
            if spec.kind == TransformKind::Chain {
                let Some(steps) = spec.steps.as_ref() else {
                    errors.push(ValidationDiagnostic::error(
                        format!("definitions.transforms.{name}.steps"),
                        "required",
                        "chain requires steps",
                    ));
                    continue;
                };
                if steps.is_empty() {
                    errors.push(ValidationDiagnostic::error(
                        format!("definitions.transforms.{name}.steps"),
                        "required",
                        "chain requires steps",
                    ));
                }
                for (i, step) in steps.iter().enumerate() {
                    check(
                        &registry,
                        &mut errors,
                        step,
                        &format!("definitions.transforms.{name}.steps[{i}]"),
                    );
                }
            }
        }
    }

    errors
}

fn select_eval_file_by_size(value: &Value) -> Option<Value> {
    if let Some(obj) = value.as_object() {
        if obj.contains_key("filename") {
            return Some(Value::String(
                obj.get("filename")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            ));
        }
    }
    let rows = value.as_array()?;
    let mut best: Option<(i64, &Map<String, Value>)> = None;
    let has_calibration = rows.iter().any(|r| {
        r.get("format")
            .and_then(Value::as_str)
            .is_some_and(|f| f.eq_ignore_ascii_case("calibration"))
    });
    for row in rows {
        let Some(obj) = row.as_object() else {
            continue;
        };
        let format = obj
            .get("format")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if has_calibration && format != "calibration" {
            continue;
        }
        let size = obj
            .get("filesize")
            .and_then(|v| {
                v.as_i64()
                    .or_else(|| value_string(Some(v)).and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0);
        if best.map(|(s, _)| size > s).unwrap_or(true) {
            best = Some((size, obj));
        }
    }
    best.and_then(|(_, obj)| {
        obj.get("filename")
            .cloned()
            .or_else(|| value_string(obj.get("filename")).map(Value::String))
    })
}

pub fn value_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(v) => Some(v.to_string()),
        _ => None,
    }
}

fn extract_digits(value: &Value) -> Option<String> {
    let raw = value_string(Some(value))?;
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

fn is_empty_value(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Array(items) => items.is_empty(),
        Value::Object(map) => map.is_empty(),
        Value::String(s) => s.trim().is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MappingSpec, ProjectConfig, TransformRef};

    #[test]
    fn strip_prefix_named_definition() {
        let registry = TransformRegistry {
            named: BTreeMap::from([(
                "hipass_source_name".into(),
                TransformSpec {
                    kind: "strip_prefix".into(),
                    prefix: Some("HIPASS".into()),
                    suffix: None,
                    separators: None,
                    pattern: None,
                    group: None,
                    from: None,
                    to: None,
                    default: None,
                    steps: None,
                },
            )]),
        };
        let out = registry
            .apply_named("hipass_source_name", &json!("HIPASSJ1313-15"))
            .unwrap();
        assert_eq!(out, json!("J1313-15"));
    }

    #[test]
    fn legacy_strip_hipass_alias() {
        let registry = TransformRegistry::from_config(&ProjectConfig::default());
        let out = registry
            .apply_named_with_legacy("strip_hipass_prefix", &json!("HIPASSJ1313-15"))
            .unwrap();
        assert_eq!(out, json!("J1313-15"));
    }

    #[test]
    fn trim_lowercase_replace() {
        let registry = TransformRegistry {
            named: BTreeMap::from([
                (
                    "trimmed".into(),
                    TransformSpec {
                        kind: "trim".into(),
                        prefix: None,
                        suffix: None,
                        separators: None,
                        pattern: None,
                        group: None,
                        from: None,
                        to: None,
                        default: None,
                        steps: None,
                    },
                ),
                (
                    "lower".into(),
                    TransformSpec {
                        kind: "lowercase".into(),
                        prefix: None,
                        suffix: None,
                        separators: None,
                        pattern: None,
                        group: None,
                        from: None,
                        to: None,
                        default: None,
                        steps: None,
                    },
                ),
                (
                    "normalize_dash".into(),
                    TransformSpec {
                        kind: "replace".into(),
                        prefix: None,
                        suffix: None,
                        separators: None,
                        pattern: None,
                        group: None,
                        from: Some("_".into()),
                        to: Some("-".into()),
                        default: None,
                        steps: None,
                    },
                ),
            ]),
        };
        assert_eq!(
            registry.apply_named("trimmed", &json!("  x  ")).unwrap(),
            json!("x")
        );
        assert_eq!(
            registry.apply_named("lower", &json!("AbC")).unwrap(),
            json!("abc")
        );
        assert_eq!(
            registry
                .apply_named("normalize_dash", &json!("a_b"))
                .unwrap(),
            json!("a-b")
        );
    }

    #[test]
    fn chain_named_and_inline() {
        let registry = TransformRegistry {
            named: BTreeMap::from([
                (
                    "askap_sbid".into(),
                    TransformSpec {
                        kind: "extract_digits".into(),
                        prefix: None,
                        suffix: None,
                        separators: None,
                        pattern: None,
                        group: None,
                        from: None,
                        to: None,
                        default: None,
                        steps: None,
                    },
                ),
                (
                    "normalized_sbid".into(),
                    TransformSpec {
                        kind: "chain".into(),
                        prefix: None,
                        suffix: None,
                        separators: None,
                        pattern: None,
                        group: None,
                        from: None,
                        to: None,
                        default: None,
                        steps: Some(vec!["askap_sbid".into(), "trim".into()]),
                    },
                ),
                (
                    "trim".into(),
                    TransformSpec {
                        kind: "trim".into(),
                        prefix: None,
                        suffix: None,
                        separators: None,
                        pattern: None,
                        group: None,
                        from: None,
                        to: None,
                        default: None,
                        steps: None,
                    },
                ),
            ]),
        };
        let out = registry
            .apply_named("normalized_sbid", &json!(" ASKAP-123 "))
            .unwrap();
        assert_eq!(out, json!("123"));
        let inline = apply_field_transform(
            &registry,
            &MappingSpec {
                from: "obs_id".into(),
                transform: Some(TransformRef::Chain(vec![
                    "askap_sbid".into(),
                    "trim".into(),
                ])),
            },
            &json!(" ASKAP-456 "),
        )
        .unwrap();
        assert_eq!(inline, json!("456"));
    }

    #[test]
    fn default_if_empty() {
        let spec = TransformSpec {
            kind: "default_if_empty".into(),
            prefix: None,
            suffix: None,
            separators: None,
            pattern: None,
            group: None,
            from: None,
            to: None,
            default: Some("fallback".into()),
            steps: None,
        };
        assert_eq!(
            apply_transform_spec(&spec, &json!("")).unwrap(),
            json!("fallback")
        );
        assert_eq!(
            apply_transform_spec(&spec, &json!("value")).unwrap(),
            json!("value")
        );
    }

    #[test]
    fn build_template_context_from_source_identity() {
        let yaml = r#"
apiVersion: beampipe.dev/v2
kind: ProjectConfig
metadata:
  id: test
definitions:
  transforms:
    hipass_source_name:
      kind: strip_prefix
      prefix: HIPASS
source_identity:
  canonical: source_identifier
  template_vars:
    source_identifier:
      from: canonical
    source_name:
      transform: hipass_source_name
"#;
        let config = ProjectConfig::from_slice(yaml.as_bytes()).unwrap();
        let ctx = build_template_context("HIPASSJ1313-15", &config);
        assert_eq!(ctx["source_identifier"], json!("HIPASSJ1313-15"));
        assert_eq!(ctx["source_name"], json!("J1313-15"));
    }

    #[test]
    fn validate_unknown_transform_fails() {
        let yaml = r#"
apiVersion: beampipe.dev/v2
kind: ProjectConfig
metadata:
  id: test
adapters:
  required: [casda]
discovery:
  prepare_metadata:
    field_map:
      sbid:
        from: obs_id
        transform: does_not_exist
"#;
        let config = ProjectConfig::from_slice(yaml.as_bytes()).unwrap();
        let errors = validate_transform_refs(&config);
        assert!(errors.iter().any(|e| e.message.contains("does_not_exist")));
    }

    #[test]
    fn validate_inline_chain_refs() {
        let yaml = r#"
apiVersion: beampipe.dev/v2
kind: ProjectConfig
metadata:
  id: test
adapters:
  required: [casda]
definitions:
  transforms:
    askap_sbid:
      kind: extract_digits
discovery:
  prepare_metadata:
    field_map:
      sbid:
        from: obs_id
        transform: [askap_sbid, missing_step]
"#;
        let config = ProjectConfig::from_slice(yaml.as_bytes()).unwrap();
        let errors = validate_transform_refs(&config);
        assert!(errors.iter().any(|e| e.message.contains("missing_step")));
    }
}
