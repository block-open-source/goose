# Building goose for RISC-V (riscv64gc-unknown-linux-gnu)

This document describes how to build goose-cli for RISC-V 64-bit systems with full V8/code-mode support.

> [!WARNING]
> This is an experimental, community-contributed build process. RISC-V is not
> officially supported by the goose project.

## Prerequisites

- Rust 1.96.1+ (no toolchain upgrade needed)
- RISC-V cross-compilation toolchain: `gcc-riscv64-linux-gnu`
- Target: `rustup target add riscv64gc-unknown-linux-gnu`

### Linker Configuration

Building **on** RISC-V hardware needs nothing extra - the native linker is
used automatically.

**Cross-compiling** (e.g. from x86_64) needs the cross linker, otherwise
rustc invokes the host `cc` and linking fails. Either export:

```bash
export CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER=riscv64-linux-gnu-gcc
```

or add to `.cargo/config.toml` (or `~/.cargo/config.toml`):

```toml
[target.riscv64gc-unknown-linux-gnu]
linker = "riscv64-linux-gnu-gcc"
```

Adjust the binary name if your distribution ships the cross gcc under a
different name.

## Overview

V8 152.2.0 is the first version with pre-built RISC-V binaries. However:
- Current goose uses v8 145.0.0 via deno_core 0.381.1 (no RISC-V support)
- Upgrading requires patching deno_core and serde_v8 for V8 152 API changes
- These patches are intrusive and affect all platforms if applied via Cargo.toml patches

## Setup Steps

### 1. Clone rusty_v8 v152.2.0

Run from the repository root:

```bash
git init vendor/rusty_v8
git -C vendor/rusty_v8 remote add origin https://github.com/denoland/rusty_v8.git
git -C vendor/rusty_v8 fetch --depth 1 origin 2768994f664e8a6e3aba27503606c58339136e2a
git -C vendor/rusty_v8 checkout --detach FETCH_HEAD
rm -rf vendor/rusty_v8/.git
```

This provides v8 152.2.0 with RISC-V support. Cargo will download pre-built binaries automatically.

### 2. Vendor deno_core 0.381.1

Run from the repository root:

```bash
wget -P vendor https://static.crates.io/crates/deno_core/deno_core-0.381.1.crate
echo "77660b04f5368bcd69a42e0fdd6343e325eb781ec7d79bbf49ff2548ae4f17b6  vendor/deno_core-0.381.1.crate" | sha256sum --check
tar -C vendor -xzf vendor/deno_core-0.381.1.crate
mv vendor/deno_core-0.381.1 vendor/deno_core
rm vendor/deno_core-0.381.1.crate
```

### 3. Vendor serde_v8 0.290.0

Run from the repository root:

```bash
wget -P vendor https://static.crates.io/crates/serde_v8/serde_v8-0.290.0.crate
echo "21a52aca5a5661aea9c2ccf8c2f985f2381266d7aab03a0d02beadb4ae1190ea  vendor/serde_v8-0.290.0.crate" | sha256sum --check
tar -C vendor -xzf vendor/serde_v8-0.290.0.crate
mv vendor/serde_v8-0.290.0 vendor/serde_v8
rm vendor/serde_v8-0.290.0.crate
```

### 4. Apply Patches

#### vendor/v8 — drop it from the workspace

The cloned `vendor/rusty_v8` is itself the `v8` crate at `152.2.0`, so the
root `[patch.crates-io]` entry points straight at it (see step 6). The
committed `vendor/v8` shim is not edited, but it must leave the workspace:
it pulls in `v8-goose`, which declares `links = "rusty_v8"`, and once the
patch resolves denoland's `v8` 152 (also `links = "rusty_v8"`) a workspace
containing both fails with "more than one crate with links=rusty_v8". Remove
`vendor/v8` from `members` and add it to `exclude` (see step 6).

#### vendor/deno_core/Cargo.toml

Update the v8 and ICU-data dependencies:

```toml
[dependencies.v8]
version = "152.2.0"  # was 145.0.0
default-features = false

[dependencies.deno_core_icudata]
version = "0.78.0"  # was 0.77.0; V8 152 needs the ICU 78 data blob
optional = true
```

#### vendor/serde_v8/Cargo.toml

Update v8 dependency:

```toml
[dependencies.v8]
version = "152.2.0"  # was 145.0.0
default-features = false
```

### 5. Source Patches to vendor/deno_core

These patches adapt deno_core 0.381.1 to V8 152 API changes.

#### Wrap `v8::Global::open()` calls in `unsafe {}`

V8 152 marked `Global::open()` as unsafe. Affected files:
- `error.rs` (2 locations)
- `modules/map.rs` (9 locations)
- `runtime/jsrealm.rs` (1 location)
- `runtime/jsruntime.rs` (3 locations)
- `ops_builtin_v8.rs` (1 location)

Example:
```rust
// Before:
let cb = callback.open(scope);

// After:
let cb = unsafe { callback.open(scope) };
```

#### Disable fast-call path in `runtime/bindings.rs`

Fast-call API causes SIGILL on RISC-V during snapshot creation:

```rust
// Replace lines ~600-605:
let template = builder.build(scope);
// (Remove the if/else with fast_function)
```

#### Update `WasmStreaming` in `ops_builtin.rs`

```rust
// Before:
pub struct WasmStreamingResource(pub(crate) RefCell<v8::WasmStreaming>);

// After:
pub struct WasmStreamingResource(pub(crate) RefCell<v8::WasmStreaming<false>>);
```

#### Update ICU data in `runtime/setup.rs`

The initializer name and the data blob must match (see the
`deno_core_icudata` bump above) — `set_common_data_78` rejects the ICU 77
blob and V8 initialization fails.

```rust
// Line ~19:
v8::icu::set_common_data_78(deno_core_icudata::ICU_DATA).unwrap();
// was: set_common_data_77

// Line ~24 - remove this line:
" --no-validate-asm",  // Remove - V8 13+ doesn't support this flag
```

### 6. Update Root Cargo.toml

Add workspace exclusion and patches:

```toml
[workspace]
members = ["crates/*"]  # drop "vendor/v8" (see step 4)
exclude = ["vendor/v8", "vendor/rusty_v8"]
resolver = "2"

# Relax ICU pins (temporal 0.2.6 handles this):
icu_calendar = { version = ">=2.1", default-features = false }
icu_locale = { version = ">=2.1", default-features = false }

[patch.crates-io]
deno_core = { path = "vendor/deno_core" }
serde_v8 = { path = "vendor/serde_v8" }
v8 = { path = "vendor/rusty_v8" }  # was vendor/v8; rusty_v8 is the v8 crate
# ... existing patches
```

### 7. Update crates/goose/Cargo.toml

Relax ICU pins:

```toml
icu_calendar = { version = ">=2.1", default-features = false }
icu_locale = { version = ">=2.1", default-features = false }
```

### 8. RISC-V Update Command Handling (already in repo)

`crates/goose-cli/src/commands/update.rs` already handles RISC-V:

- `asset_name()` includes the riscv64gc-gnu asset name so the function
  compiles on RISC-V (otherwise every branch is cfg-disabled and the body
  would evaluate to `()` instead of `&'static str`).
- `update()` bails early on riscv64 with a clear error, because release
  artifacts are not published for this platform yet:

```rust
#[cfg(all(target_arch = "riscv64", not(feature = "disable-update")))]
{
    bail!("Self-update is not supported on riscv64: no release artifacts are published for this platform.");
}
```

Once RISC-V assets are added to the release pipeline, remove that bail
block and drop `not(target_arch = "riscv64")` from the main branch.

Because the bail makes the rest of the module cfg-disabled on riscv64,
the file also carries a module-level allow so `clippy -D warnings` stays
clean for RISC-V builds (inert on other targets):

```rust
#![cfg_attr(target_arch = "riscv64", allow(dead_code, unused_imports, unused_variables))]
```

### 9. Update Dependencies

Update only the dependencies this setup changes, not the whole lockfile:

```bash
cargo update deno_core_icudata icu_calendar icu_locale temporal_rs
```

Expected changes:
- v8: 145.0.0 → 152.2.0 (local, via the patch)
- deno_core_icudata: 0.77.0 → 0.78.0
- temporal_rs: 0.1.2 → 0.2.6
- icu_calendar: 2.1.1 → 2.3.0

### 10. Build

```bash
cargo build --release --target riscv64gc-unknown-linux-gnu -p goose-cli --bin goose
```

Output: `target/riscv64gc-unknown-linux-gnu/release/goose`

## Complete Patch Script

See `scripts/setup-riscv.sh` for automated setup.

## Known Issues

- Fast-call optimization disabled (SIGILL on RISC-V snapshot creation)
- Patches affect all platforms when using vendored deno_core
- Adds ~2.6MB vendor dependencies to repository

## Alternative: Conditional Compilation

For production PR, consider:
1. Keep deno_core/serde_v8 patches in separate git branch
2. Document manual setup steps
3. Only commit minimal changes (update.rs, CI workflow)
4. Let users apply patches locally for RISC-V builds

## Verification

```bash
# Check architecture
file target/riscv64gc-unknown-linux-gnu/release/goose
# Output: ELF 64-bit LSB pie executable, UCB RISC-V

# Test execution (on RISC-V hardware)
./target/riscv64gc-unknown-linux-gnu/release/goose --version
./target/riscv64gc-unknown-linux-gnu/release/goose doctor
```
