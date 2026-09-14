# goose-sdk

Python bindings for the goose Development Kit (GDK).

This package is generated from the Rust `goose-sdk` crate using UniFFI.

## Build a local wheel

From the repository root:

```bash
just --justfile crates/goose-sdk/justfile python-wheel
```

The wheel is written to `crates/goose-sdk/python/dist/`.
