use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    propagation::TraceContextPropagator,
    runtime,
    trace::{RandomIdGenerator, Sampler, Tracer, TracerProvider as SdkTracerProvider},
    Resource,
};
use std::sync::{Arc, OnceLock};

static OTEL_INIT: OnceLock<()> = OnceLock::new();
static SDK_TRACER_PROVIDER: OnceLock<Arc<SdkTracerProvider>> = OnceLock::new();

/// SDK tracer for `tracing-opentelemetry` bridge (after `init_if_enabled`).
pub fn sdk_tracer() -> Option<Tracer> {
    SDK_TRACER_PROVIDER.get().map(|p| p.tracer("beampipe-v2"))
}

/// Install OTLP trace export when BEAMPIPE_OTEL_ENABLED=true.
pub fn init_if_enabled() {
    OTEL_INIT.get_or_init(|| {
        let enabled = std::env::var("BEAMPIPE_OTEL_ENABLED")
            .ok()
            .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes"));
        if !enabled {
            return;
        }
        let endpoint = std::env::var("BEAMPIPE_OTEL_ENDPOINT")
            .unwrap_or_else(|_| "http://127.0.0.1:4317".into());
        let service =
            std::env::var("BEAMPIPE_OTEL_SERVICE_NAME").unwrap_or_else(|_| "beampipe-v2".into());

        global::set_text_map_propagator(TraceContextPropagator::new());

        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint.clone())
            .build()
            .expect("failed to build OTLP span exporter");

        let ratio = std::env::var("BEAMPIPE_OTEL_SAMPLER_RATIO")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(1.0);
        let sampler = if ratio >= 1.0 {
            Sampler::AlwaysOn
        } else if ratio <= 0.0 {
            Sampler::AlwaysOff
        } else {
            Sampler::TraceIdRatioBased(ratio)
        };
        let tracer_provider = SdkTracerProvider::builder()
            .with_batch_exporter(exporter, runtime::Tokio)
            .with_sampler(sampler)
            .with_id_generator(RandomIdGenerator::default())
            .with_resource(Resource::new(vec![opentelemetry::KeyValue::new(
                "service.name",
                service.clone(),
            )]))
            .build();

        let provider = Arc::new(tracer_provider);
        global::set_tracer_provider(provider.as_ref().clone());
        let _ = SDK_TRACER_PROVIDER.set(provider);
        eprintln!("beampipe: OTLP trace export enabled endpoint={endpoint} service={service}");
    });
}
