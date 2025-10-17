use anyhow::{Context, Result};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use opentelemetry::global;
use opentelemetry::trace::TracerProvider;
use opentelemetry::KeyValue;
use opentelemetry_sdk::resource::Resource;
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use opentelemetry_stdout::SpanExporter;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub struct Telemetry {
    metrics_handle: PrometheusHandle,
    tracer_provider: Option<SdkTracerProvider>,
}

impl Telemetry {
    pub fn init() -> Result<Self> {
        let metrics_handle = PrometheusBuilder::new()
            .install_recorder()
            .context("failed to install Prometheus metrics recorder")?;

        let mut tracer_provider = None;

        if env_truthy("BEACH_LIFEGUARD_OTEL_STDOUT") {
            let exporter = SpanExporter::default();
            let provider = SdkTracerProvider::builder()
                .with_simple_exporter(exporter)
                .with_sampler(Sampler::AlwaysOn)
                .with_resource(
                    Resource::builder()
                        .with_attributes(vec![KeyValue::new("service.name", "beach-lifeguard")])
                        .build(),
                )
                .build();

            let tracer = provider.tracer("beach-lifeguard");

            global::set_tracer_provider(provider.clone());
            tracing_subscriber::registry()
                .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
                .with(tracing_subscriber::fmt::layer().with_target(false))
                .with(tracing_opentelemetry::layer().with_tracer(tracer))
                .try_init()
                .context("failed to initialise tracing subscriber")?;
            tracer_provider = Some(provider);
        } else {
            tracing_subscriber::registry()
                .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
                .with(tracing_subscriber::fmt::layer().with_target(false))
                .try_init()
                .context("failed to initialise tracing subscriber")?;
        }

        if tracer_provider.is_some() {
            info!("OpenTelemetry stdout exporter enabled (BEACH_LIFEGUARD_OTEL_STDOUT=1)");
        }

        Ok(Self {
            metrics_handle,
            tracer_provider,
        })
    }

    pub fn metrics_handle(&self) -> PrometheusHandle {
        self.metrics_handle.clone()
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        if let Some(provider) = &self.tracer_provider {
            if let Err(err) = provider.shutdown() {
                warn!(
                    error = %err,
                    "failed to shutdown OpenTelemetry tracer provider"
                );
            }
        }
    }
}

fn env_truthy(key: &str) -> bool {
    match std::env::var(key) {
        Ok(val) => matches!(
            val.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}
