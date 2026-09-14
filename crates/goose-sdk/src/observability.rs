//! Structured provider request/response observability for GDK callers.
//!
//! Kotlin and Python consumers register an [`ObservabilityHook`] to receive
//! typed lifecycle events (start, response metadata, completion) for both
//! streaming and non-streaming provider calls instead of scraping logs.
//!
//! Hooks are opt-in: with no hook registered nothing is emitted and no work is
//! performed on the request path. The hook is invoked synchronously, wrapped in
//! `catch_unwind` so a throwing foreign callback cannot fail the request.

use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, RwLock,
    },
    time::Instant,
};

use goose_providers::conversation::message::Message;
use rmcp::model::Tool;

use crate::bindings::{GooseError, GooseStreamError, Usage};

/// Receives structured provider request lifecycle events.
#[uniffi::export(callback_interface)]
pub trait ObservabilityHook: Send + Sync {
    fn on_request_start(&self, event: RequestStartEvent);
    fn on_response_start(&self, event: ResponseStartEvent);
    fn on_request_end(&self, event: RequestEndEvent);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RequestOperation {
    Complete,
    Stream,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct RequestPayload {
    pub system: String,
    pub messages_json: String,
    pub tools_json: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct RequestStartEvent {
    pub request_id: String,
    pub provider: String,
    pub model: String,
    pub operation: RequestOperation,
    pub payload: Option<RequestPayload>,
}

/// Emitted when the provider response becomes available: when the stream is
/// opened for streaming requests, or when the body is received otherwise.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ResponseStartEvent {
    pub request_id: String,
    pub provider: String,
    pub model: String,
    pub operation: RequestOperation,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct RequestEndEvent {
    pub request_id: String,
    pub provider: String,
    pub model: String,
    pub operation: RequestOperation,
    pub outcome: RequestOutcome,
    pub duration_ms: u64,
    pub usage: Option<Usage>,
    pub response_json: Option<String>,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum RequestOutcome {
    Success,
    Failure { error: GooseStreamError },
}

static HOOK: RwLock<Option<Arc<RegisteredHook>>> = RwLock::new(None);
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Registers the process-wide observability hook, replacing any previous one.
///
/// Payloads are omitted unless `capture_payloads` is enabled because system
/// prompts, conversations and tool results routinely contain sensitive data.
#[uniffi::export(default(capture_payloads = false))]
pub fn set_observability_hook(hook: Box<dyn ObservabilityHook>, capture_payloads: bool) {
    let registered = Arc::new(RegisteredHook {
        hook,
        capture_payloads,
        revoked: AtomicBool::new(false),
    });
    let previous = HOOK
        .write()
        .expect("observability hook lock")
        .replace(registered);
    if let Some(previous) = previous {
        previous.revoke();
    }
}

/// Removes the observability hook, after which no further events are emitted,
/// including for requests that are still in flight.
#[uniffi::export]
pub fn clear_observability_hook() {
    if let Some(previous) = HOOK.write().expect("observability hook lock").take() {
        previous.revoke();
    }
}

struct RegisteredHook {
    hook: Box<dyn ObservabilityHook>,
    capture_payloads: bool,
    revoked: AtomicBool,
}

impl RegisteredHook {
    fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
    }

    fn emit(&self, deliver: impl FnOnce(&dyn ObservabilityHook)) {
        if self.revoked.load(Ordering::Acquire) {
            return;
        }
        let _ = catch_unwind(AssertUnwindSafe(|| deliver(self.hook.as_ref())));
    }
}

pub(crate) struct RequestDescriptor<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    pub operation: RequestOperation,
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [Tool],
}

/// Tracks one provider request and emits its lifecycle events. Disabled (and
/// free) when no hook is registered.
pub(crate) struct RequestObserver {
    active: Option<ActiveRequest>,
}

struct ActiveRequest {
    hook: Arc<RegisteredHook>,
    request_id: String,
    provider: String,
    model: String,
    operation: RequestOperation,
    started: Instant,
    ended: AtomicBool,
}

impl RequestObserver {
    pub(crate) fn start(descriptor: RequestDescriptor<'_>) -> Self {
        let Some(hook) = HOOK.read().expect("observability hook lock").clone() else {
            return Self { active: None };
        };

        let request_id = format!("req-{}", NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed));
        let payload = hook.capture_payloads.then(|| RequestPayload {
            system: descriptor.system.to_string(),
            messages_json: serde_json::to_string(descriptor.messages)
                .unwrap_or_else(|_| "null".to_string()),
            tools_json: serde_json::to_string(descriptor.tools)
                .unwrap_or_else(|_| "null".to_string()),
        });

        let event = RequestStartEvent {
            request_id: request_id.clone(),
            provider: descriptor.provider.to_string(),
            model: descriptor.model.to_string(),
            operation: descriptor.operation,
            payload,
        };
        hook.emit(|hook| hook.on_request_start(event));

        Self {
            active: Some(ActiveRequest {
                hook,
                request_id,
                provider: descriptor.provider.to_string(),
                model: descriptor.model.to_string(),
                operation: descriptor.operation,
                started: Instant::now(),
                ended: AtomicBool::new(false),
            }),
        }
    }

    pub(crate) fn captures_payloads(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.hook.capture_payloads)
    }

    pub(crate) fn response_started(&self) {
        let Some(active) = self.active.as_ref() else {
            return;
        };

        let event = ResponseStartEvent {
            request_id: active.request_id.clone(),
            provider: active.provider.clone(),
            model: active.model.clone(),
            operation: active.operation,
            elapsed_ms: active.started.elapsed().as_millis() as u64,
        };
        active.hook.emit(|hook| hook.on_response_start(event));
    }

    pub(crate) fn succeeded(&self, usage: Option<Usage>, response_json: Option<String>) {
        self.end(RequestOutcome::Success, usage, response_json);
    }

    pub(crate) fn fail(&self, error: GooseError) -> GooseError {
        self.end(
            RequestOutcome::Failure {
                error: GooseStreamError::from(&error),
            },
            None,
            None,
        );
        error
    }

    pub(crate) fn fail_stream(&self, error: GooseStreamError) {
        self.end(RequestOutcome::Failure { error }, None, None);
    }

    fn end(&self, outcome: RequestOutcome, usage: Option<Usage>, response_json: Option<String>) {
        let Some(active) = self.active.as_ref() else {
            return;
        };

        if active.ended.swap(true, Ordering::AcqRel) {
            return;
        }

        let event = RequestEndEvent {
            request_id: active.request_id.clone(),
            provider: active.provider.clone(),
            model: active.model.clone(),
            operation: active.operation,
            outcome,
            duration_ms: active.started.elapsed().as_millis() as u64,
            usage,
            response_json,
        };
        active.hook.emit(|hook| hook.on_request_end(event));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    static HOOK_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Default)]
    struct Recorder {
        events: Mutex<Vec<Event>>,
        panic_on_start: bool,
    }

    #[derive(Debug, Clone)]
    enum Event {
        Start(RequestStartEvent),
        ResponseStart(ResponseStartEvent),
        End(RequestEndEvent),
    }

    impl ObservabilityHook for Arc<Recorder> {
        fn on_request_start(&self, event: RequestStartEvent) {
            assert!(!self.panic_on_start, "foreign hook exploded");
            self.push(Event::Start(event));
        }

        fn on_response_start(&self, event: ResponseStartEvent) {
            self.push(Event::ResponseStart(event));
        }

        fn on_request_end(&self, event: RequestEndEvent) {
            self.push(Event::End(event));
        }
    }

    impl Recorder {
        fn push(&self, event: Event) {
            self.events.lock().unwrap().push(event);
        }
    }

    /// Serializes access to the process-wide hook and clears it on drop.
    struct RegisteredRecorder {
        recorder: Arc<Recorder>,
        _lock: MutexGuard<'static, ()>,
    }

    impl RegisteredRecorder {
        fn new(recorder: Recorder, capture_payloads: bool) -> Self {
            let lock = HOOK_TEST_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let recorder = Arc::new(recorder);
            set_observability_hook(Box::new(Arc::clone(&recorder)), capture_payloads);
            Self {
                recorder,
                _lock: lock,
            }
        }

        fn events(&self) -> Vec<Event> {
            self.recorder.events.lock().unwrap().clone()
        }
    }

    impl Drop for RegisteredRecorder {
        fn drop(&mut self) {
            clear_observability_hook();
        }
    }

    fn descriptor() -> RequestDescriptor<'static> {
        RequestDescriptor {
            provider: "databricks",
            model: "claude-sonnet-4",
            operation: RequestOperation::Stream,
            system: "you are helpful",
            messages: &[],
            tools: &[],
        }
    }

    fn usage() -> Usage {
        Usage {
            input_tokens: Some(11),
            output_tokens: Some(7),
            total_tokens: Some(18),
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
            reasoning_tokens: None,
            model: "claude-sonnet-4".to_string(),
            provider_metadata_json: None,
            additional_data_json: None,
        }
    }

    #[test]
    fn success_emits_start_response_and_end_with_usage() {
        let registered = RegisteredRecorder::new(Recorder::default(), false);
        let observer = RequestObserver::start(descriptor());
        observer.response_started();
        observer.succeeded(Some(usage()), None);

        let events = registered.events();
        assert_eq!(events.len(), 3);
        let Event::Start(start) = &events[0] else {
            panic!("expected start, got {:?}", events[0]);
        };
        assert_eq!(start.provider, "databricks");
        assert_eq!(start.model, "claude-sonnet-4");
        assert_eq!(start.operation, RequestOperation::Stream);
        assert!(start.payload.is_none());

        let Event::ResponseStart(response_start) = &events[1] else {
            panic!("expected response start, got {:?}", events[1]);
        };
        assert_eq!(response_start.request_id, start.request_id);

        let Event::End(end) = &events[2] else {
            panic!("expected end, got {:?}", events[2]);
        };
        assert_eq!(end.request_id, start.request_id);
        assert_eq!(end.usage.as_ref().unwrap().total_tokens, Some(18));
        assert!(matches!(end.outcome, RequestOutcome::Success));
        assert!(end.response_json.is_none());
    }

    #[test]
    fn failure_reports_typed_error() {
        let registered = RegisteredRecorder::new(Recorder::default(), false);
        let observer = RequestObserver::start(descriptor());
        observer.fail(GooseError::RateLimited {
            retry_after_ms: Some(1_500),
            retry_after_suffix: String::new(),
        });

        let events = registered.events();
        let Event::End(end) = &events[1] else {
            panic!("expected end, got {:?}", events[1]);
        };
        let RequestOutcome::Failure { error } = &end.outcome else {
            panic!("expected failure outcome");
        };
        assert!(matches!(
            error.kind,
            crate::bindings::GooseStreamErrorKind::RateLimited
        ));
        assert_eq!(error.retry_after_ms, Some(1_500));
        assert!(end.usage.is_none());
    }

    #[test]
    fn payload_capture_is_opt_in() {
        let registered = RegisteredRecorder::new(Recorder::default(), true);
        let observer = RequestObserver::start(descriptor());
        assert!(observer.captures_payloads());
        observer.succeeded(None, Some("{\"role\":\"assistant\"}".to_string()));

        let events = registered.events();
        let Event::Start(start) = &events[0] else {
            panic!("expected start");
        };
        let payload = start.payload.as_ref().expect("payload captured");
        assert_eq!(payload.system, "you are helpful");
        assert_eq!(payload.messages_json, "[]");
        let Event::End(end) = &events[1] else {
            panic!("expected end");
        };
        assert_eq!(
            end.response_json.as_deref(),
            Some("{\"role\":\"assistant\"}")
        );
    }

    #[test]
    fn panicking_hook_does_not_stop_later_events() {
        let registered = RegisteredRecorder::new(
            Recorder {
                panic_on_start: true,
                ..Default::default()
            },
            false,
        );
        let observer = RequestObserver::start(descriptor());
        observer.response_started();
        observer.succeeded(None, None);

        let events = registered.events();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], Event::ResponseStart(_)));
        assert!(matches!(events[1], Event::End(_)));
    }

    #[test]
    fn end_is_emitted_at_most_once() {
        let registered = RegisteredRecorder::new(Recorder::default(), false);
        let observer = RequestObserver::start(descriptor());
        observer.fail(GooseError::Timeout {
            details: "request timed out after 5ms".to_string(),
        });
        observer.succeeded(Some(usage()), None);
        observer.fail_stream(GooseStreamError {
            kind: crate::bindings::GooseStreamErrorKind::Generic,
            message: "later read".to_string(),
            retry_after_ms: None,
        });

        let events = registered.events();
        assert_eq!(events.len(), 2);
        let Event::End(end) = &events[1] else {
            panic!("expected end, got {:?}", events[1]);
        };
        let RequestOutcome::Failure { error } = &end.outcome else {
            panic!("expected failure outcome");
        };
        assert!(matches!(
            error.kind,
            crate::bindings::GooseStreamErrorKind::Timeout
        ));
    }

    #[test]
    fn clearing_hook_mid_request_stops_in_flight_events() {
        let registered = RegisteredRecorder::new(Recorder::default(), true);
        let observer = RequestObserver::start(descriptor());
        clear_observability_hook();

        observer.response_started();
        observer.succeeded(Some(usage()), Some("{\"role\":\"assistant\"}".to_string()));

        let events = registered.events();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], Event::Start(_)));
    }

    #[test]
    fn replacing_hook_stops_in_flight_events_to_the_old_hook() {
        let registered = RegisteredRecorder::new(Recorder::default(), false);
        let observer = RequestObserver::start(descriptor());
        set_observability_hook(Box::new(Arc::new(Recorder::default())), false);

        observer.succeeded(None, None);

        let events = registered.events();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], Event::Start(_)));
    }

    #[test]
    fn cleared_hook_produces_no_events() {
        let registered = RegisteredRecorder::new(Recorder::default(), true);
        clear_observability_hook();

        let observer = RequestObserver::start(descriptor());
        assert!(!observer.captures_payloads());
        observer.response_started();
        observer.succeeded(Some(usage()), None);

        assert!(registered.events().is_empty());
    }
}
