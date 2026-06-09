use std::sync::OnceLock;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Registry};

static TRACING_INIT: OnceLock<()> = OnceLock::new();

/// Install env-filter + optional JSON fmt + OpenTelemetry bridge when OTEL is enabled.
pub fn init_subscriber() {
    TRACING_INIT.get_or_init(|| {
        crate::otel::init_if_enabled();
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let json = std::env::var("BEAMPIPE_LOG_JSON")
            .ok()
            .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes"));
        let otel_enabled = std::env::var("BEAMPIPE_OTEL_ENABLED")
            .ok()
            .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes"));

        if otel_enabled {
            let Some(tracer) = crate::otel::sdk_tracer() else {
                eprintln!("beampipe: OTEL enabled but tracer provider not initialized");
                return;
            };
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            if json {
                Registry::default()
                    .with(filter)
                    .with(otel_layer)
                    .with(tracing_subscriber::fmt::layer().json())
                    .init();
            } else {
                Registry::default()
                    .with(filter)
                    .with(otel_layer)
                    .with(tracing_subscriber::fmt::layer())
                    .init();
            }
        } else if json {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .json()
                .init();
        } else {
            tracing_subscriber::fmt().with_env_filter(filter).init();
        }
    });
}
