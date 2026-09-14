# UniFFI examples

These examples exercise the in-process GDK UniFFI bindings from Python and Kotlin.

## Prerequisites

```bash
source bin/activate-hermit
```

The Python example uses the declarative DeepSeek provider:

```bash
export DEEPSEEK_API_KEY=...
```

The Kotlin/JVM Maven smoke test uses the native OpenAI provider:

```bash
export OPENAI_API_KEY=...
```

## Generate bindings

Regenerate the Python bindings before running the Python example:

```bash
just --justfile crates/goose-sdk/justfile _generate python
```

This writes generated bindings and the debug native library under `crates/goose-sdk/generated/`.

## Python provider example

```bash
DYLD_LIBRARY_PATH=target/debug LD_LIBRARY_PATH=target/debug \
  uv run --script crates/goose-sdk/examples/uniffi/provider.py
```

## Kotlin/JVM Maven smoke test

Build the local Maven artifact and run the downstream smoke test app:

```bash
just --justfile crates/goose-sdk/justfile maven-package
cd crates/goose-sdk/examples/uniffi/kotlin
gradle --no-daemon run
```

Or run the same flow through the goose-sdk justfile:

```bash
just --justfile crates/goose-sdk/justfile kotlin
```

The Kotlin example consumes the local Maven artifact `io.github.aaif-goose:gdk` from `mavenLocal()` and imports the generated package namespace `io.github.aaif_goose`.

On newer JDKs, the example enables native access with `--enable-native-access=ALL-UNNAMED` because the GDK uses JNA to load the bundled native library.
