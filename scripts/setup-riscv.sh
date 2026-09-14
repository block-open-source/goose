#!/usr/bin/env bash
# Experimental, community-contributed RISC-V setup. This configuration is not
# officially supported by the goose project.
# This script vendors dependencies and applies necessary patches for V8 152.2.0.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
VENDOR_DIR="$REPO_ROOT/vendor"
RUSTY_V8_COMMIT="2768994f664e8a6e3aba27503606c58339136e2a"
DENO_CORE_SHA256="77660b04f5368bcd69a42e0fdd6343e325eb781ec7d79bbf49ff2548ae4f17b6"
SERDE_V8_SHA256="21a52aca5a5661aea9c2ccf8c2f985f2381266d7aab03a0d02beadb4ae1190ea"

verify_sha256() {
    local file="$1"
    local expected="$2"
    local actual

    if command -v sha256sum > /dev/null 2>&1; then
        actual="$(sha256sum "$file" | awk '{print $1}')"
    elif command -v shasum > /dev/null 2>&1; then
        actual="$(shasum -a 256 "$file" | awk '{print $1}')"
    else
        echo "Error: sha256sum or shasum is required to verify downloads" >&2
        return 1
    fi

    if [ "$actual" != "$expected" ]; then
        echo "Error: SHA-256 mismatch for $file" >&2
        echo "Expected: $expected" >&2
        echo "Actual:   $actual" >&2
        return 1
    fi
}

echo "=== Setting up goose for RISC-V build ==="
echo "Repository: $REPO_ROOT"
echo ""

# 1. Clone rusty_v8
echo "1. Cloning rusty_v8 commit $RUSTY_V8_COMMIT (v152.2.0)..."
if [ -d "$VENDOR_DIR/rusty_v8" ]; then
    echo "   Already exists, skipping..."
else
    git -C "$VENDOR_DIR" init rusty_v8
    git -C "$VENDOR_DIR/rusty_v8" remote add origin https://github.com/denoland/rusty_v8.git
    git -C "$VENDOR_DIR/rusty_v8" fetch --depth 1 origin "$RUSTY_V8_COMMIT"
    git -C "$VENDOR_DIR/rusty_v8" checkout --detach FETCH_HEAD
    rm -rf "$VENDOR_DIR/rusty_v8/.git"
    echo "   ✓ Done"
fi

# 2. Vendor deno_core
echo "2. Vendoring deno_core 0.381.1..."
if [ -d "$VENDOR_DIR/deno_core" ]; then
    echo "   Already exists, skipping..."
else
    cd "$VENDOR_DIR"
    wget -q https://static.crates.io/crates/deno_core/deno_core-0.381.1.crate
    verify_sha256 deno_core-0.381.1.crate "$DENO_CORE_SHA256"
    tar xzf deno_core-0.381.1.crate
    mv deno_core-0.381.1 deno_core
    rm deno_core-0.381.1.crate
    echo "   ✓ Done"
fi

# 3. Vendor serde_v8
echo "3. Vendoring serde_v8 0.290.0..."
if [ -d "$VENDOR_DIR/serde_v8" ]; then
    echo "   Already exists, skipping..."
else
    cd "$VENDOR_DIR"
    wget -q https://static.crates.io/crates/serde_v8/serde_v8-0.290.0.crate
    verify_sha256 serde_v8-0.290.0.crate "$SERDE_V8_SHA256"
    tar xzf serde_v8-0.290.0.crate
    mv serde_v8-0.290.0 serde_v8
    rm serde_v8-0.290.0.crate
    echo "   ✓ Done"
fi

# 4. vendor/v8
# Nothing to patch: the cloned vendor/rusty_v8 IS the `v8` crate at 152.2.0,
# so step 8 points `[patch.crates-io] v8` straight at it. The committed
# vendor/v8 shim is left alone (it stays a workspace member for cargo-machete
# but nothing depends on it once the patch is redirected).
echo "4. vendor/v8: using cloned rusty_v8 directly, nothing to patch"

# 5. Update deno_core v8 dependency
echo "5. Updating deno_core v8 dependency to 152.2.0..."
sed -i 's/version = "145\.0\.0"/version = "152.2.0"/' "$VENDOR_DIR/deno_core/Cargo.toml"
echo "   ✓ Done"

# 6. Update serde_v8 v8 dependency
echo "6. Updating serde_v8 v8 dependency to 152.2.0..."
sed -i 's/version = "145\.0\.0"/version = "152.2.0"/' "$VENDOR_DIR/serde_v8/Cargo.toml"
echo "   ✓ Done"

# 7. Apply deno_core source patches
echo "7. Applying deno_core source patches for V8 152 API..."

# Wrap .open() calls in unsafe
sed -i 's/let format_exception_cb = format_exception_cb\.open(scope);/let format_exception_cb = unsafe { format_exception_cb.open(scope) };/' "$VENDOR_DIR/deno_core/error.rs"
sed -i 's/let cb = cb\.open(tc_scope);/let cb = unsafe { cb.open(tc_scope) };/' "$VENDOR_DIR/deno_core/error.rs"
sed -i 's/let resolver = resolver_handle\.open(scope);/let resolver = unsafe { resolver_handle.open(scope) };/g' "$VENDOR_DIR/deno_core/modules/map.rs"
sed -i 's/let module = module_handle\.open(scope);/let module = unsafe { module_handle.open(scope) };/g' "$VENDOR_DIR/deno_core/modules/map.rs"
sed -i 's/let promise = pending_dyn_evaluate\.promise\.open(scope);/let promise = unsafe { pending_dyn_evaluate.promise.open(scope) };/g' "$VENDOR_DIR/deno_core/modules/map.rs"
sed -i 's/let _module = pending_dyn_evaluate\.module\.open(scope);/let _module = unsafe { pending_dyn_evaluate.module.open(scope) };/g' "$VENDOR_DIR/deno_core/modules/map.rs"
sed -i 's/let resolver = state\.resolver\.open(scope);/let resolver = unsafe { state.resolver.open(scope) };/g' "$VENDOR_DIR/deno_core/modules/map.rs"
sed -i 's/let ctx = self\.context()\.open(scope);/let ctx = unsafe { self.context().open(scope) };/g' "$VENDOR_DIR/deno_core/runtime/jsrealm.rs"
sed -i 's/let cb = function\.open(scope);/let cb = unsafe { function.open(scope) };/g' "$VENDOR_DIR/deno_core/runtime/jsruntime.rs"
sed -i 's/run_immediate_callbacks_cb\.as_ref()\.unwrap()\.open(tc_scope);/unsafe { run_immediate_callbacks_cb.as_ref().unwrap().open(tc_scope) };/g' "$VENDOR_DIR/deno_core/runtime/jsruntime.rs"
sed -i 's/let function = handler\.open(scope);/let function = unsafe { handler.open(scope) };/g' "$VENDOR_DIR/deno_core/runtime/jsruntime.rs"
sed -i 's/js_event_loop_tick_cb\.as_ref()\.unwrap()\.open(tc_scope);/unsafe { js_event_loop_tick_cb.as_ref().unwrap().open(tc_scope) };/g' "$VENDOR_DIR/deno_core/runtime/jsruntime.rs"

# Fix ops_builtin_v8.rs - multiline sed
sed -i '/cb_handle$/{ N; s/cb_handle\n      \.open(scope)/unsafe { cb_handle.open(scope) }/; }' "$VENDOR_DIR/deno_core/ops_builtin_v8.rs"

# Disable fast-call path in bindings.rs: replace the whole if/else block
# with the fallback assignment (c on an address range replaces all of it)
sed -i '/let template = if let Some(fast_function)/,/};/c\
  // Disable fast path for v8 152 - causes SIGILL on RISC-V snapshot creation\
  let template = builder.build(scope);' "$VENDOR_DIR/deno_core/runtime/bindings.rs"

# With the fast path disabled, the fast_fn binding above is unused; silence
# the resulting warning so clippy -D warnings stays clean.
sed -i 's/^  let (slow_fn, fast_fn) = /  let (slow_fn, _fast_fn) = /' "$VENDOR_DIR/deno_core/runtime/bindings.rs"

# Update WasmStreaming generic
sed -i 's/pub struct WasmStreamingResource(pub(crate) RefCell<v8::WasmStreaming>);/pub struct WasmStreamingResource(pub(crate) RefCell<v8::WasmStreaming<false>>);/' "$VENDOR_DIR/deno_core/ops_builtin.rs"

# ICU 77 -> 78: V8 152 bundles ICU 78, so the initializer name AND the data
# blob must both move. deno_core_icudata 0.78.0 ships the matching ICU 78 data;
# passing the 0.77.0 blob to set_common_data_78 fails V8 initialization.
sed -i 's/set_common_data_77/set_common_data_78/' "$VENDOR_DIR/deno_core/runtime/setup.rs"
sed -i 's/^version = "0\.77\.0"$/version = "0.78.0"/' "$VENDOR_DIR/deno_core/Cargo.toml"

# Remove --no-validate-asm flag
sed -i 's/" --no-validate-asm",//' "$VENDOR_DIR/deno_core/runtime/setup.rs"

echo "   ✓ Done"

# 8. Update root Cargo.toml
echo "8. Updating root Cargo.toml..."
cd "$REPO_ROOT"

# Drop the vendor/v8 shim from the workspace and exclude it plus rusty_v8.
# The shim pulls in `v8-goose`, which declares `links = "rusty_v8"`; once the
# patch below resolves denoland's `v8` 152 (also `links = "rusty_v8"`), a
# workspace containing both fails with "more than one crate with
# links=rusty_v8". Use awk for portability (GNU/BSD sed multi-line differs).
if ! grep -q '^exclude = \[' Cargo.toml; then
    awk '
        /^members = \[/ { in_members = 1 }
        in_members && /cargo-machete/ { next }
        in_members && /"vendor\/v8"/ { next }
        in_members && /^\]$/ {
            print
            print "exclude = [\"vendor/v8\", \"vendor/rusty_v8\"]"
            in_members = 0
            next
        }
        { print }
    ' Cargo.toml > Cargo.toml.new
    mv Cargo.toml.new Cargo.toml
fi

# Relax ICU pins
sed -i 's/icu_calendar = { version = "=2\.1\.1"/icu_calendar = { version = ">=2.1"/' Cargo.toml
sed -i 's/icu_locale = { version = "=2\.1\.1"/icu_locale = { version = ">=2.1"/' Cargo.toml

# Add deno_core/serde_v8 patches and repoint the existing `v8` patch at the
# cloned rusty_v8 (same crate name, version 152.2.0). Multi-line sed `a\` is
# fragile across sed implementations, so use awk.
if ! grep -q 'vendor/deno_core' Cargo.toml; then
    awk '
        /^\[patch\.crates-io\]$/ {
            print
            print "deno_core = { path = \"vendor/deno_core\" }"
            print "serde_v8 = { path = \"vendor/serde_v8\" }"
            next
        }
        /^v8 = \{ path = "vendor\/v8" \}$/ {
            print "v8 = { path = \"vendor/rusty_v8\" }"
            next
        }
        { print }
    ' Cargo.toml > Cargo.toml.new
    mv Cargo.toml.new Cargo.toml
fi

echo "   ✓ Done"

# 9. Update crates/goose/Cargo.toml
echo "9. Updating crates/goose/Cargo.toml..."
sed -i 's/icu_calendar = { version = "=2\.1\.1"/icu_calendar = { version = ">=2.1"/' "$REPO_ROOT/crates/goose/Cargo.toml"
sed -i 's/icu_locale = { version = "=2\.1\.1"/icu_locale = { version = ">=2.1"/' "$REPO_ROOT/crates/goose/Cargo.toml"
echo "   ✓ Done"

# Note: update.rs already handles RISC-V (asset name + self-update bail) in
# the repo, so no patching is needed here.

# 10. Update dependencies
# Re-resolve only what this setup changes: the ICU-78 data blob and the
# ICU/temporal crates freed by the relaxed pins. The patched path crates
# (v8, deno_core, serde_v8) are picked up by the build itself. A bare
# `cargo update` would drift every eligible dependency.
echo "10. Updating Cargo dependencies..."
cd "$REPO_ROOT"
cargo update --quiet deno_core_icudata icu_calendar icu_locale temporal_rs
echo "   ✓ Done"

# 11. Verify toolchain is available.
echo "11. Checking RISC-V toolchain..."
if [ "$(uname -m)" = "riscv64" ]; then
    echo "   ✓ native RISC-V build - no cross toolchain needed"
elif command -v riscv64-linux-gnu-gcc > /dev/null 2>&1; then
    echo "   ✓ riscv64-linux-gnu-gcc found"
    echo "     For cross-compiling, export before building:"
    echo "     export CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER=riscv64-linux-gnu-gcc"
else
    echo "   ⚠ riscv64-linux-gnu-gcc not found"
    echo "     Install it (e.g. 'sudo apt install gcc-riscv64-linux-gnu') and set:"
    echo "     export CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER=riscv64-linux-gnu-gcc"
fi

echo ""
echo "=== Setup complete! ==="
echo ""
echo "To build for RISC-V:"
echo "  cargo build --release --target riscv64gc-unknown-linux-gnu -p goose-cli --bin goose"
echo ""
echo "Output binary:"
echo "  target/riscv64gc-unknown-linux-gnu/release/goose"
