//! Tracing + OpenTelemetry plumbing for adk-rs.
//!
//! Call [`init`] once at process start. Pass a [`TelemetryConfig`] to control
//! the log level, format, and (with the `otel` feature) an OTLP endpoint.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::Registry;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::Layered;
use tracing_subscriber::prelude::*;

type FilteredRegistry = Layered<EnvFilter, Registry>;
type BoxedLayer = Box<dyn Layer<FilteredRegistry> + Send + Sync>;

use adk_rs_error::Result;

/// Output format for the local stderr log layer.
#[derive(Debug, Clone, Copy, Default)]
pub enum LogFormat {
    /// Human-friendly compact output (default).
    #[default]
    Compact,
    /// Pretty multi-line output.
    Pretty,
    /// Newline-delimited JSON (good for log aggregators).
    Json,
}

/// Telemetry configuration.
#[derive(Debug, Clone, Default)]
pub struct TelemetryConfig {
    /// `RUST_LOG`-style filter (default: `info`).
    pub filter: Option<String>,
    /// stderr output format.
    pub format: LogFormat,
    /// OTLP HTTP endpoint URL (only used when the `otel` feature is on).
    pub otlp_endpoint: Option<String>,
    /// Service name for OTel resource (default: `adk-rs`).
    pub service_name: Option<String>,
}

/// Initialize tracing. Idempotent — second calls are no-ops.
pub fn init(cfg: TelemetryConfig) -> Result<()> {
    static INITIALIZED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if INITIALIZED.get().is_some() {
        return Ok(());
    }

    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(cfg.filter.as_deref().unwrap_or("info")))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let fmt_layer: BoxedLayer = match cfg.format {
        LogFormat::Compact => fmt::layer().compact().boxed(),
        LogFormat::Pretty => fmt::layer().pretty().boxed(),
        LogFormat::Json => fmt::layer().json().boxed(),
    };

    let mut layers: Vec<BoxedLayer> = vec![fmt_layer];

    #[cfg(feature = "otel")]
    if let Some(otel) = build_otel_layer(&cfg)? {
        layers.push(otel);
    }

    tracing_subscriber::registry()
        .with(filter)
        .with(layers)
        .try_init()
        .ok();

    let _ = INITIALIZED.set(());
    Ok(())
}

#[cfg(feature = "otel")]
fn build_otel_layer(cfg: &TelemetryConfig) -> Result<Option<BoxedLayer>> {
    let Some(endpoint) = cfg.otlp_endpoint.clone() else {
        return Ok(None);
    };

    use opentelemetry::KeyValue;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::runtime;

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()
        .map_err(|e| adk_rs_error::Error::other(format!("OTLP exporter: {e}")))?;
    let resource = Resource::new(vec![KeyValue::new(
        "service.name",
        cfg.service_name.clone().unwrap_or_else(|| "adk-rs".into()),
    )]);
    let provider = opentelemetry_sdk::trace::TracerProvider::builder()
        .with_batch_exporter(exporter, runtime::Tokio)
        .with_resource(resource)
        .build();
    let tracer = opentelemetry::trace::TracerProvider::tracer(&provider, "adk-rs");
    Ok(Some(
        tracing_opentelemetry::layer().with_tracer(tracer).boxed(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent() {
        let cfg = TelemetryConfig::default();
        init(cfg.clone()).unwrap();
        init(cfg).unwrap();
    }
}
