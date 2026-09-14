# goose-sdk

The bindings layer for goose, published as the goose Development Kit (GDK). It
houses the shared types used for both ACP and GDK access, and exposes a
cross-language version of the goose API.

With `--features uniffi` the crate compiles to native bindings for Python and
Kotlin (namespace `goose` / `io.github.aaif_goose`). The UniFFI surface lets
callers construct providers, stream provider completions, perform non-streaming
completion, and pass rich message/tool content across the FFI boundary.

```bash
just python   # build bindings + run examples/uniffi/provider.py
just kotlin   # build the Maven artifact + run examples/uniffi/kotlin
```

## Observability hooks

Register an `ObservabilityHook` to receive typed provider request lifecycle
events instead of parsing debug output. Hooks are opt-in: with no hook
registered nothing is emitted and the request path is unchanged.

Each request emits `onRequestStart`, then `onResponseStart` once the provider
response is available (the stream opens for streaming requests), then exactly
one `onRequestEnd` carrying the outcome (`Success`, or `Failure` with a typed
`GooseStreamError`), `durationMs`, and token `usage`. All three events share a
`requestId` so they can be correlated with application telemetry. A streaming
read that times out ends the trace, so continuing to read the stream afterwards
never produces a second `onRequestEnd`.

`clearObservabilityHook` also stops delivery for requests that are still in
flight, so no events reach a hook after it is cleared.

```kotlin
class TracingHook : ObservabilityHook {
    override fun onRequestStart(event: RequestStartEvent) {
        tracer.startSpan(event.requestId, event.provider, event.model)
    }

    override fun onResponseStart(event: ResponseStartEvent) {
        tracer.recordTimeToFirstByte(event.requestId, event.elapsedMs)
    }

    override fun onRequestEnd(event: RequestEndEvent) {
        tracer.finishSpan(event.requestId, event.outcome, event.durationMs, event.usage)
    }
}

setObservabilityHook(TracingHook(), capturePayloads = false)
```

Hooks are invoked synchronously on the calling thread and a throwing callback is
caught, so it cannot fail the request. Keep them fast: slow callbacks add
latency to the request they observe.

### Security guidance

`capturePayloads` defaults to `false` and payloads are omitted entirely in that
mode: `RequestStartEvent.payload` and `RequestEndEvent.responseJson` are null,
leaving only non-sensitive metadata (provider, model, latency, usage).

Enable `capturePayloads` only when you control the sink. It exposes the system
prompt, the full conversation, and tool schemas, which routinely contain
credentials, customer data, and other secrets. Apply your own redaction before
persisting or exporting these fields.

Response capture is asymmetric: `RequestEndEvent.responseJson` is populated for
`complete` calls but is always null for `stream` calls, because the streamed
response is delivered to the caller chunk by chunk and is never buffered by the
SDK. Assemble the streamed body from the chunks you already receive if you need
it.

## Python package

The PyPI package is published as `goose-sdk` and imports as `goose`.
Build a local wheel from the repository root with:

```bash
just --justfile crates/goose-sdk/justfile python-wheel
```

This regenerates the UniFFI Python bindings, copies the release native library
into the package, and writes the wheel to `crates/goose-sdk/python/dist/`.

## Maven package

The Maven Central artifact is published as `io.github.aaif-goose:gdk` and uses
the Rust crate version from `crates/goose-sdk/Cargo.toml`.

```bash
just --justfile crates/goose-sdk/justfile maven-package
```

This regenerates the UniFFI Kotlin bindings and packages them with the native
library in a JVM jar. CI builds the native libraries for supported platforms and
can optionally publish the combined artifact to Maven Central.
