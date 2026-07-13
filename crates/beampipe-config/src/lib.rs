use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use thiserror::Error;

pub const CONFIG_API_VERSION: &str = "beampipe.dev/config/v1";
pub const DEFAULT_CONFIG_FILE: &str = "beampipe.yaml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub database_url: String,
    pub bind_addr: String,
    /// `BEAMPIPE_ENV` - `development` (default) or `production` / `prod`.
    pub beampipe_env: String,
    pub jwt_secret: String,
    pub access_token_expire_minutes: i64,
    pub refresh_token_expire_days: i64,
    pub worker_poll_interval_ms: u64,
    pub worker_lock_seconds: i64,
    pub worker_heartbeat_interval_seconds: u64,
    pub worker_concurrency: u32,
    pub worker_scheduler_enabled: bool,
    pub worker_instance_name: Option<String>,
    pub worker_pool: String,
    pub worker_capabilities: Vec<String>,
    pub worker_labels: BTreeMap<String, String>,
    pub db_max_connections: u32,
    pub discovery_source_concurrency: u32,
    pub discovery_tap_health_check_enabled: bool,
    pub discovery_tap_health_timeout_seconds: u64,
    pub shaping_discovery_max_in_flight_batches: i64,
    pub shaping_discovery_max_batches_per_tick: i64,
    pub shaping_execution_max_in_flight_runs: i64,
    pub shaping_queue_max_depth: i64,
    pub shaping_enqueue_pacing_ms: u64,
    pub scheduler_interval_seconds: u64,
    pub redis_url: Option<String>,
    pub rate_limit_requests: u64,
    pub rate_limit_period_seconds: u64,
    pub metrics_bind_addr: String,
    pub metrics_server_enabled: bool,
    pub metrics_public: bool,
    pub cors_allow_origins: Option<String>,
    pub require_rate_limiter: bool,
    pub log_json: bool,
    pub otel_enabled: bool,
    pub otel_endpoint: String,
    pub otel_service_name: String,
    pub otel_sampler_ratio: f64,
    pub provenance_retention_days: i32,
    pub migrate_on_serve: bool,
    pub use_real_backends: bool,
    pub casda_tap_url: Option<String>,
    pub vizier_tap_url: Option<String>,
    pub tm_url: Option<String>,
    pub dim_url: Option<String>,
    pub slurm_remote_user: Option<String>,
    pub dim_destroy_session: bool,
    pub dim_poll_interval_seconds: Option<u64>,
    pub slurm_poll_interval_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConfigSource {
    Default,
    File { path: String },
    Environment { variable: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedSetting {
    pub key: String,
    pub source: ConfigSource,
    pub value: String,
    pub redacted: bool,
}

#[derive(Debug, Clone)]
pub struct SettingsResolution {
    pub settings: Settings,
    pub config_path: Option<PathBuf>,
    sources: BTreeMap<String, ConfigSource>,
}

impl SettingsResolution {
    pub fn source_for(&self, key: &str) -> Option<&ConfigSource> {
        self.sources.get(key)
    }

    pub fn explain(&self) -> Vec<ResolvedSetting> {
        let values = serde_json::to_value(&self.settings).unwrap_or_default();
        self.sources
            .iter()
            .map(|(key, source)| {
                let redacted = is_sensitive(key);
                let value = if redacted {
                    "<redacted>".to_string()
                } else {
                    display_json_value(values.get(key))
                };
                ResolvedSetting {
                    key: key.clone(),
                    source: source.clone(),
                    value,
                    redacted,
                }
            })
            .collect()
    }
}

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("missing required setting {0}")]
    Missing(&'static str),
    #[error("invalid environment variable {name}: {value}")]
    Invalid { name: &'static str, value: String },
    #[error("could not read config file {path}: {source}")]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid config file {path}: {source}")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
    #[error("unsupported config apiVersion '{found}'; expected '{expected}'")]
    UnsupportedApiVersion {
        found: String,
        expected: &'static str,
    },
    #[error("invalid config kind '{0}'; expected 'BeampipeConfig'")]
    InvalidKind(String),
}

impl Settings {
    /// Load defaults, an optional YAML config file, and environment overrides.
    ///
    /// `BEAMPIPE_CONFIG` (or legacy `BEAMPIPE_CONFIG_FILE`) selects a file. If neither
    /// is set, `./beampipe.yaml` is loaded when present. Existing environment variable
    /// names retain highest precedence.
    pub fn load() -> Result<SettingsResolution, SettingsError> {
        let _ = dotenvy::dotenv();
        let config_path = configured_path();
        Self::load_from_path(config_path.as_deref())
    }

    pub fn load_from_path(path: Option<&Path>) -> Result<SettingsResolution, SettingsError> {
        let file = match path {
            Some(path) => load_file(path)?,
            None => SettingsFile::default(),
        };
        let file_source = path.map(|path| path.display().to_string());
        let mut resolver = Resolver::new(file_source);

        let settings = Settings {
            database_url: resolver.required_string(
                "database_url",
                "DATABASE_URL",
                file.database.url.clone(),
            )?,
            bind_addr: resolver.string(
                "bind_addr",
                "BEAMPIPE_BIND_ADDR",
                file.api.bind_addr.clone(),
                "127.0.0.1:8080",
            ),
            beampipe_env: resolver.string(
                "beampipe_env",
                "BEAMPIPE_ENV",
                file.environment.clone(),
                "development",
            ),
            jwt_secret: resolver.string(
                "jwt_secret",
                "BEAMPIPE_JWT_SECRET",
                file.auth.jwt_secret.clone(),
                "secret-key",
            ),
            access_token_expire_minutes: resolver.parsed(
                "access_token_expire_minutes",
                "BEAMPIPE_ACCESS_TOKEN_EXPIRE_MINUTES",
                file.auth.access_token_expire_minutes,
                30,
            )?,
            refresh_token_expire_days: resolver.parsed(
                "refresh_token_expire_days",
                "BEAMPIPE_REFRESH_TOKEN_EXPIRE_DAYS",
                file.auth.refresh_token_expire_days,
                7,
            )?,
            worker_poll_interval_ms: resolver.parsed(
                "worker_poll_interval_ms",
                "BEAMPIPE_WORKER_POLL_INTERVAL_MS",
                file.worker.poll_interval_ms,
                1000,
            )?,
            worker_lock_seconds: resolver.parsed(
                "worker_lock_seconds",
                "BEAMPIPE_WORKER_LOCK_SECONDS",
                file.worker.lock_seconds,
                120,
            )?,
            worker_heartbeat_interval_seconds: resolver.parsed(
                "worker_heartbeat_interval_seconds",
                "BEAMPIPE_WORKER_HEARTBEAT_INTERVAL_SECONDS",
                file.worker.heartbeat_interval_seconds,
                10,
            )?,
            worker_concurrency: resolver.parsed(
                "worker_concurrency",
                "BEAMPIPE_WORKER_CONCURRENCY",
                file.worker.concurrency,
                1,
            )?,
            worker_scheduler_enabled: resolver.boolean(
                "worker_scheduler_enabled",
                "BEAMPIPE_WORKER_SCHEDULER_ENABLED",
                file.worker.scheduler_enabled,
                true,
            )?,
            worker_instance_name: resolver.optional_string(
                "worker_instance_name",
                "BEAMPIPE_WORKER_INSTANCE_NAME",
                file.worker.instance_name.clone(),
            ),
            worker_pool: resolver.string(
                "worker_pool",
                "BEAMPIPE_WORKER_POOL",
                file.worker.pool.clone(),
                "default",
            ),
            worker_capabilities: resolver.string_list(
                "worker_capabilities",
                "BEAMPIPE_WORKER_CAPABILITIES",
                file.worker.capabilities.clone(),
                default_worker_capabilities(),
            ),
            worker_labels: resolver.labels(
                "worker_labels",
                "BEAMPIPE_WORKER_LABELS",
                file.worker.labels.clone(),
            )?,
            db_max_connections: resolver.parsed(
                "db_max_connections",
                "BEAMPIPE_DB_MAX_CONNECTIONS",
                file.database.max_connections,
                10,
            )?,
            discovery_source_concurrency: resolver.parsed(
                "discovery_source_concurrency",
                "BEAMPIPE_DISCOVERY_SOURCE_CONCURRENCY",
                file.discovery.source_concurrency,
                5,
            )?,
            discovery_tap_health_check_enabled: resolver.boolean(
                "discovery_tap_health_check_enabled",
                "BEAMPIPE_DISCOVERY_TAP_HEALTH_CHECK_ENABLED",
                file.discovery.tap_health_check_enabled,
                true,
            )?,
            discovery_tap_health_timeout_seconds: resolver.parsed(
                "discovery_tap_health_timeout_seconds",
                "BEAMPIPE_DISCOVERY_TAP_HEALTH_TIMEOUT_SECONDS",
                file.discovery.tap_health_timeout_seconds,
                10,
            )?,
            shaping_discovery_max_in_flight_batches: resolver.parsed(
                "shaping_discovery_max_in_flight_batches",
                "BEAMPIPE_SHAPING_DISCOVERY_MAX_IN_FLIGHT_BATCHES",
                file.shaping.discovery_max_in_flight_batches,
                4,
            )?,
            shaping_discovery_max_batches_per_tick: resolver.parsed(
                "shaping_discovery_max_batches_per_tick",
                "BEAMPIPE_SHAPING_DISCOVERY_MAX_BATCHES_PER_TICK",
                file.shaping.discovery_max_batches_per_tick,
                4,
            )?,
            shaping_execution_max_in_flight_runs: resolver.parsed(
                "shaping_execution_max_in_flight_runs",
                "BEAMPIPE_SHAPING_EXECUTION_MAX_IN_FLIGHT_RUNS",
                file.shaping.execution_max_in_flight_runs,
                2,
            )?,
            shaping_queue_max_depth: resolver.parsed(
                "shaping_queue_max_depth",
                "BEAMPIPE_SHAPING_QUEUE_MAX_DEPTH",
                file.shaping.queue_max_depth,
                200,
            )?,
            shaping_enqueue_pacing_ms: resolver.parsed(
                "shaping_enqueue_pacing_ms",
                "BEAMPIPE_SHAPING_ENQUEUE_PACING_MS",
                file.shaping.enqueue_pacing_ms,
                0,
            )?,
            scheduler_interval_seconds: resolver.parsed(
                "scheduler_interval_seconds",
                "BEAMPIPE_SCHEDULER_INTERVAL_SECONDS",
                file.scheduler.interval_seconds,
                60,
            )?,
            redis_url: resolver.optional_string(
                "redis_url",
                "BEAMPIPE_REDIS_URL",
                file.redis.url.clone(),
            ),
            rate_limit_requests: resolver.parsed(
                "rate_limit_requests",
                "BEAMPIPE_RATE_LIMIT_REQUESTS",
                file.api.rate_limit_requests,
                10,
            )?,
            rate_limit_period_seconds: resolver.parsed(
                "rate_limit_period_seconds",
                "BEAMPIPE_RATE_LIMIT_PERIOD_SECONDS",
                file.api.rate_limit_period_seconds,
                3600,
            )?,
            metrics_bind_addr: resolver.string(
                "metrics_bind_addr",
                "BEAMPIPE_METRICS_BIND_ADDR",
                file.metrics.bind_addr.clone(),
                "127.0.0.1:9090",
            ),
            metrics_server_enabled: resolver.boolean(
                "metrics_server_enabled",
                "BEAMPIPE_METRICS_SERVER_ENABLED",
                file.metrics.server_enabled,
                true,
            )?,
            metrics_public: resolver.boolean(
                "metrics_public",
                "BEAMPIPE_METRICS_PUBLIC",
                file.metrics.public,
                false,
            )?,
            cors_allow_origins: resolver.optional_string(
                "cors_allow_origins",
                "BEAMPIPE_CORS_ALLOW_ORIGINS",
                file.api.cors_allow_origins.clone(),
            ),
            require_rate_limiter: resolver.boolean(
                "require_rate_limiter",
                "BEAMPIPE_REQUIRE_RATE_LIMITER",
                file.api.require_rate_limiter,
                false,
            )?,
            log_json: resolver.boolean(
                "log_json",
                "BEAMPIPE_LOG_JSON",
                file.telemetry.log_json,
                false,
            )?,
            otel_enabled: resolver.boolean(
                "otel_enabled",
                "BEAMPIPE_OTEL_ENABLED",
                file.telemetry.otel_enabled,
                false,
            )?,
            otel_endpoint: resolver.string(
                "otel_endpoint",
                "BEAMPIPE_OTEL_ENDPOINT",
                file.telemetry.otel_endpoint.clone(),
                "http://127.0.0.1:4317",
            ),
            otel_service_name: resolver.string(
                "otel_service_name",
                "BEAMPIPE_OTEL_SERVICE_NAME",
                file.telemetry.otel_service_name.clone(),
                "beampipe-v2",
            ),
            otel_sampler_ratio: resolver.parsed(
                "otel_sampler_ratio",
                "BEAMPIPE_OTEL_SAMPLER_RATIO",
                file.telemetry.otel_sampler_ratio,
                1.0,
            )?,
            provenance_retention_days: resolver.parsed(
                "provenance_retention_days",
                "BEAMPIPE_PROVENANCE_RETENTION_DAYS",
                file.maintenance.provenance_retention_days,
                90,
            )?,
            migrate_on_serve: resolver.boolean(
                "migrate_on_serve",
                "BEAMPIPE_MIGRATE_ON_SERVE",
                file.maintenance.migrate_on_serve,
                false,
            )?,
            use_real_backends: resolver.boolean(
                "use_real_backends",
                "BEAMPIPE_USE_REAL_BACKENDS",
                file.integrations.use_real_backends,
                false,
            )?,
            casda_tap_url: resolver.optional_string(
                "casda_tap_url",
                "BEAMPIPE_CASDA_TAP_URL",
                file.integrations.casda_tap_url.clone(),
            ),
            vizier_tap_url: resolver.optional_string(
                "vizier_tap_url",
                "BEAMPIPE_VIZIER_TAP_URL",
                file.integrations.vizier_tap_url.clone(),
            ),
            tm_url: resolver.optional_string(
                "tm_url",
                "BEAMPIPE_TM_URL",
                file.integrations.tm_url.clone(),
            ),
            dim_url: resolver.optional_string(
                "dim_url",
                "BEAMPIPE_DIM_URL",
                file.integrations.dim_url.clone(),
            ),
            slurm_remote_user: resolver.optional_string(
                "slurm_remote_user",
                "SLURM_REMOTE_USER",
                file.integrations.slurm_remote_user.clone(),
            ),
            dim_destroy_session: resolver.boolean(
                "dim_destroy_session",
                "BEAMPIPE_DIM_DESTROY_SESSION",
                file.integrations.dim_destroy_session,
                false,
            )?,
            dim_poll_interval_seconds: resolver.optional_parsed(
                "dim_poll_interval_seconds",
                "BEAMPIPE_DIM_POLL_INTERVAL_SECONDS",
                file.integrations.dim_poll_interval_seconds,
            )?,
            slurm_poll_interval_seconds: resolver.optional_parsed(
                "slurm_poll_interval_seconds",
                "BEAMPIPE_SLURM_POLL_INTERVAL_SECONDS",
                file.integrations.slurm_poll_interval_seconds,
            )?,
        };
        validate_settings(&settings)?;
        Ok(SettingsResolution {
            settings,
            config_path: path.map(Path::to_path_buf),
            sources: resolver.sources,
        })
    }

    /// Compatibility entry point used by existing API, scheduler, and worker startup.
    pub fn from_env() -> Result<Self, SettingsError> {
        Ok(Self::load()?.settings)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsFile {
    #[serde(rename = "apiVersion")]
    api_version: Option<String>,
    kind: Option<String>,
    environment: Option<String>,
    #[serde(default)]
    database: DatabaseFile,
    #[serde(default)]
    api: ApiFile,
    #[serde(default)]
    auth: AuthFile,
    #[serde(default)]
    worker: WorkerFile,
    #[serde(default)]
    discovery: DiscoveryFile,
    #[serde(default)]
    shaping: ShapingFile,
    #[serde(default)]
    scheduler: SchedulerFile,
    #[serde(default)]
    redis: RedisFile,
    #[serde(default)]
    metrics: MetricsFile,
    #[serde(default)]
    telemetry: TelemetryFile,
    #[serde(default)]
    maintenance: MaintenanceFile,
    #[serde(default)]
    integrations: IntegrationsFile,
}

macro_rules! config_section {
    ($name:ident { $($field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Debug, Clone, Default, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct $name {
            $(#[serde(default)] $field: Option<$ty>,)*
        }
    };
}

config_section!(DatabaseFile {
    url: String,
    max_connections: u32,
});
config_section!(ApiFile {
    bind_addr: String,
    cors_allow_origins: String,
    rate_limit_requests: u64,
    rate_limit_period_seconds: u64,
    require_rate_limiter: bool,
});
config_section!(AuthFile {
    jwt_secret: String,
    access_token_expire_minutes: i64,
    refresh_token_expire_days: i64,
});

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerFile {
    poll_interval_ms: Option<u64>,
    lock_seconds: Option<i64>,
    heartbeat_interval_seconds: Option<u64>,
    concurrency: Option<u32>,
    scheduler_enabled: Option<bool>,
    instance_name: Option<String>,
    pool: Option<String>,
    #[serde(default)]
    capabilities: Option<Vec<String>>,
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
}

config_section!(DiscoveryFile {
    source_concurrency: u32,
    tap_health_check_enabled: bool,
    tap_health_timeout_seconds: u64,
});
config_section!(ShapingFile {
    discovery_max_in_flight_batches: i64,
    discovery_max_batches_per_tick: i64,
    execution_max_in_flight_runs: i64,
    queue_max_depth: i64,
    enqueue_pacing_ms: u64,
});
config_section!(SchedulerFile {
    interval_seconds: u64,
});
config_section!(RedisFile { url: String });
config_section!(MetricsFile {
    bind_addr: String,
    server_enabled: bool,
    public: bool,
});
config_section!(TelemetryFile {
    log_json: bool,
    otel_enabled: bool,
    otel_endpoint: String,
    otel_service_name: String,
    otel_sampler_ratio: f64,
});
config_section!(MaintenanceFile {
    provenance_retention_days: i32,
    migrate_on_serve: bool,
});
config_section!(IntegrationsFile {
    use_real_backends: bool,
    casda_tap_url: String,
    vizier_tap_url: String,
    tm_url: String,
    dim_url: String,
    slurm_remote_user: String,
    dim_destroy_session: bool,
    dim_poll_interval_seconds: u64,
    slurm_poll_interval_seconds: u64,
});

fn configured_path() -> Option<PathBuf> {
    std::env::var("BEAMPIPE_CONFIG")
        .ok()
        .or_else(|| std::env::var("BEAMPIPE_CONFIG_FILE").ok())
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            let default = PathBuf::from(DEFAULT_CONFIG_FILE);
            default.exists().then_some(default)
        })
}

fn load_file(path: &Path) -> Result<SettingsFile, SettingsError> {
    let bytes = std::fs::read(path).map_err(|source| SettingsError::ConfigRead {
        path: path.to_path_buf(),
        source,
    })?;
    let file: SettingsFile =
        serde_yaml::from_slice(&bytes).map_err(|source| SettingsError::ConfigParse {
            path: path.to_path_buf(),
            source,
        })?;
    let api_version = file.api_version.as_deref().unwrap_or_default();
    if api_version != CONFIG_API_VERSION {
        return Err(SettingsError::UnsupportedApiVersion {
            found: api_version.to_string(),
            expected: CONFIG_API_VERSION,
        });
    }
    if let Some(kind) = file.kind.as_deref() {
        if kind != "BeampipeConfig" {
            return Err(SettingsError::InvalidKind(kind.to_string()));
        }
    }
    Ok(file)
}

struct Resolver {
    file_path: Option<String>,
    sources: BTreeMap<String, ConfigSource>,
}

impl Resolver {
    fn new(file_path: Option<String>) -> Self {
        Self {
            file_path,
            sources: BTreeMap::new(),
        }
    }

    fn record(&mut self, key: &str, source: ConfigSource) {
        self.sources.insert(key.to_string(), source);
    }

    fn file_source(&self) -> ConfigSource {
        ConfigSource::File {
            path: self
                .file_path
                .clone()
                .unwrap_or_else(|| DEFAULT_CONFIG_FILE.to_string()),
        }
    }

    fn required_string(
        &mut self,
        key: &str,
        env: &'static str,
        file: Option<String>,
    ) -> Result<String, SettingsError> {
        let value = self.optional_string(key, env, file);
        value
            .filter(|value| !value.trim().is_empty())
            .ok_or(SettingsError::Missing(env))
    }

    fn string(
        &mut self,
        key: &str,
        env: &'static str,
        file: Option<String>,
        default: &str,
    ) -> String {
        if let Ok(value) = std::env::var(env) {
            self.record(
                key,
                ConfigSource::Environment {
                    variable: env.to_string(),
                },
            );
            return value;
        }
        if let Some(value) = file {
            self.record(key, self.file_source());
            return value;
        }
        self.record(key, ConfigSource::Default);
        default.to_string()
    }

    fn optional_string(
        &mut self,
        key: &str,
        env: &'static str,
        file: Option<String>,
    ) -> Option<String> {
        if let Ok(value) = std::env::var(env) {
            self.record(
                key,
                ConfigSource::Environment {
                    variable: env.to_string(),
                },
            );
            return (!value.trim().is_empty()).then_some(value);
        }
        if file.is_some() {
            self.record(key, self.file_source());
            return file.filter(|value| !value.trim().is_empty());
        }
        self.record(key, ConfigSource::Default);
        None
    }

    fn parsed<T>(
        &mut self,
        key: &str,
        env: &'static str,
        file: Option<T>,
        default: T,
    ) -> Result<T, SettingsError>
    where
        T: std::str::FromStr,
    {
        if let Ok(value) = std::env::var(env) {
            self.record(
                key,
                ConfigSource::Environment {
                    variable: env.to_string(),
                },
            );
            return value
                .parse::<T>()
                .map_err(|_| SettingsError::Invalid { name: env, value });
        }
        if let Some(value) = file {
            self.record(key, self.file_source());
            return Ok(value);
        }
        self.record(key, ConfigSource::Default);
        Ok(default)
    }

    fn optional_parsed<T>(
        &mut self,
        key: &str,
        env: &'static str,
        file: Option<T>,
    ) -> Result<Option<T>, SettingsError>
    where
        T: std::str::FromStr,
    {
        if let Ok(value) = std::env::var(env) {
            self.record(
                key,
                ConfigSource::Environment {
                    variable: env.to_string(),
                },
            );
            return value
                .parse::<T>()
                .map(Some)
                .map_err(|_| SettingsError::Invalid { name: env, value });
        }
        if file.is_some() {
            self.record(key, self.file_source());
            return Ok(file);
        }
        self.record(key, ConfigSource::Default);
        Ok(None)
    }

    fn boolean(
        &mut self,
        key: &str,
        env: &'static str,
        file: Option<bool>,
        default: bool,
    ) -> Result<bool, SettingsError> {
        if let Ok(value) = std::env::var(env) {
            self.record(
                key,
                ConfigSource::Environment {
                    variable: env.to_string(),
                },
            );
            return parse_bool(env, value);
        }
        if let Some(value) = file {
            self.record(key, self.file_source());
            return Ok(value);
        }
        self.record(key, ConfigSource::Default);
        Ok(default)
    }

    fn string_list(
        &mut self,
        key: &str,
        env: &'static str,
        file: Option<Vec<String>>,
        default: Vec<String>,
    ) -> Vec<String> {
        if let Ok(value) = std::env::var(env) {
            self.record(
                key,
                ConfigSource::Environment {
                    variable: env.to_string(),
                },
            );
            return split_csv(&value);
        }
        if let Some(value) = file {
            self.record(key, self.file_source());
            return value;
        }
        self.record(key, ConfigSource::Default);
        default
    }

    fn labels(
        &mut self,
        key: &str,
        env: &'static str,
        file: Option<BTreeMap<String, String>>,
    ) -> Result<BTreeMap<String, String>, SettingsError> {
        if let Ok(value) = std::env::var(env) {
            self.record(
                key,
                ConfigSource::Environment {
                    variable: env.to_string(),
                },
            );
            return parse_labels(env, &value);
        }
        if let Some(value) = file {
            self.record(key, self.file_source());
            return Ok(value);
        }
        self.record(key, ConfigSource::Default);
        Ok(BTreeMap::new())
    }
}

fn parse_bool(name: &'static str, value: String) -> Result<bool, SettingsError> {
    if value == "1" || value.eq_ignore_ascii_case("true") {
        Ok(true)
    } else if value == "0" || value.eq_ignore_ascii_case("false") {
        Ok(false)
    } else {
        Err(SettingsError::Invalid { name, value })
    }
}

fn parse_labels(
    name: &'static str,
    value: &str,
) -> Result<BTreeMap<String, String>, SettingsError> {
    let mut labels = BTreeMap::new();
    for item in split_csv(value) {
        let Some((key, value)) = item.split_once('=') else {
            return Err(SettingsError::Invalid { name, value: item });
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            return Err(SettingsError::Invalid { name, value: item });
        }
        labels.insert(key.to_string(), value.to_string());
    }
    Ok(labels)
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn default_worker_capabilities() -> Vec<String> {
    [
        "casda-discovery",
        "manifest-generation",
        "daliuge-translation",
        "daliuge-deployment",
        "slurm-remote",
        "output-verification",
    ]
    .into_iter()
    .map(ToString::to_string)
    .collect()
}

fn validate_settings(settings: &Settings) -> Result<(), SettingsError> {
    if settings.worker_lock_seconds <= 0 {
        return Err(SettingsError::Invalid {
            name: "BEAMPIPE_WORKER_LOCK_SECONDS",
            value: settings.worker_lock_seconds.to_string(),
        });
    }
    if settings.worker_heartbeat_interval_seconds == 0
        || settings.worker_heartbeat_interval_seconds as i64 >= settings.worker_lock_seconds
    {
        return Err(SettingsError::Invalid {
            name: "BEAMPIPE_WORKER_HEARTBEAT_INTERVAL_SECONDS",
            value: settings.worker_heartbeat_interval_seconds.to_string(),
        });
    }
    if settings.db_max_connections == 0 {
        return Err(SettingsError::Invalid {
            name: "BEAMPIPE_DB_MAX_CONNECTIONS",
            value: settings.db_max_connections.to_string(),
        });
    }
    if settings.worker_concurrency == 0 {
        return Err(SettingsError::Invalid {
            name: "BEAMPIPE_WORKER_CONCURRENCY",
            value: settings.worker_concurrency.to_string(),
        });
    }
    if !(0.0..=1.0).contains(&settings.otel_sampler_ratio) {
        return Err(SettingsError::Invalid {
            name: "BEAMPIPE_OTEL_SAMPLER_RATIO",
            value: settings.otel_sampler_ratio.to_string(),
        });
    }
    Ok(())
}

fn display_json_value(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(value)) => value.clone(),
        Some(value) => value.to_string(),
        None => "null".to_string(),
    }
}

fn is_sensitive(key: &str) -> bool {
    matches!(key, "database_url" | "jwt_secret" | "redis_url")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn strict_file_model_rejects_unknown_fields() {
        let error = serde_yaml::from_str::<SettingsFile>(
            "apiVersion: beampipe.dev/config/v1\nunknown: true\n",
        )
        .expect_err("unknown field should fail");
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn environment_has_highest_precedence() {
        let _guard = ENV_LOCK.lock().unwrap();
        const NAME: &str = "BEAMPIPE_CONFIG_TEST_PRECEDENCE";
        std::env::set_var(NAME, "7");
        let mut resolver = Resolver::new(Some("test.yaml".into()));
        let value = resolver.parsed("test", NAME, Some(3_u64), 1).unwrap();
        std::env::remove_var(NAME);
        assert_eq!(value, 7);
        assert_eq!(
            resolver.sources.get("test"),
            Some(&ConfigSource::Environment {
                variable: NAME.to_string()
            })
        );
    }

    #[test]
    fn label_parser_is_strict() {
        assert_eq!(
            parse_labels("BEAMPIPE_WORKER_LABELS", "site=pawsey,facility=setonix")
                .unwrap()
                .len(),
            2
        );
        assert!(parse_labels("BEAMPIPE_WORKER_LABELS", "site").is_err());
    }

    #[test]
    fn sensitive_values_are_identified() {
        assert!(is_sensitive("jwt_secret"));
        assert!(is_sensitive("database_url"));
        assert!(!is_sensitive("bind_addr"));
    }
}
