use opentelemetry::global;
use opentelemetry::metrics::{Meter, MeterProvider};
use opentelemetry::InstrumentationScope;
use std::env;
use std::sync::Arc;

struct SavedMeterProvider(Arc<dyn MeterProvider + Send + Sync>);

impl MeterProvider for SavedMeterProvider {
    fn meter_with_scope(&self, scope: InstrumentationScope) -> Meter {
        self.0.meter_with_scope(scope)
    }
}

pub struct OtelTestGuard {
    pub _env: env_lock::EnvGuard<'static>,
    prev_tracer: global::GlobalTracerProvider,
    prev_meter: Arc<dyn MeterProvider + Send + Sync>,
}

impl Drop for OtelTestGuard {
    fn drop(&mut self) {
        global::set_tracer_provider(self.prev_tracer.clone());
        global::set_meter_provider(SavedMeterProvider(self.prev_meter.clone()));
    }
}

pub fn clear_otel_env(overrides: &[(&'static str, &'static str)]) -> OtelTestGuard {
    let mut keys: Vec<&'static str> = vec![
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
        "OTEL_EXPORTER_OTLP_LOGS_PROTOCOL",
        "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
        "OTEL_EXPORTER_OTLP_METRICS_PROTOCOL",
        "OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE",
        "OTEL_EXPORTER_OTLP_PROTOCOL",
        "OTEL_EXPORTER_OTLP_TIMEOUT",
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
        "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT",
        "OTEL_LOG_LEVEL",
        "OTEL_LOGS_EXPORTER",
        "OTEL_METRICS_EXPORTER",
        "OTEL_RESOURCE_ATTRIBUTES",
        "OTEL_SDK_DISABLED",
        "OTEL_SERVICE_NAME",
        "OTEL_TRACES_EXPORTER",
    ];
    for &(k, _) in overrides {
        if !keys.contains(&k) {
            keys.push(k);
        }
    }

    let guard = env_lock::lock_env(keys.into_iter().map(|k| (k, None::<&str>)));
    let prev_tracer = global::tracer_provider();
    let prev_meter = global::meter_provider();
    for &(k, v) in overrides {
        env::set_var(k, v);
    }
    OtelTestGuard {
        _env: guard,
        prev_tracer,
        prev_meter,
    }
}
