use opentelemetry::global;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    propagation::TraceContextPropagator,
    runtime,
    trace::{RandomIdGenerator, Sampler, TracerProvider},
    Resource,
};
use std::sync::OnceLock;

static OTEL_INIT: OnceLock<()> = OnceLock::new();

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

        let tracer_provider = TracerProvider::builder()
            .with_batch_exporter(exporter, runtime::Tokio)
            .with_sampler(Sampler::AlwaysOn)
            .with_id_generator(RandomIdGenerator::default())
            .with_resource(Resource::new(vec![opentelemetry::KeyValue::new(
                "service.name",
                service.clone(),
            )]))
            .build();

        global::set_tracer_provider(tracer_provider);
        eprintln!("beampipe: OTLP trace export enabled endpoint={endpoint} service={service}");
    });
}
