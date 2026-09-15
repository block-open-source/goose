# Justfile

# list all tasks
default:
  @just --list

# Run all style checks and formatting (precommit validation)
check-everything:
    @echo "🔧 RUNNING ALL STYLE CHECKS..."
    @echo "  → Formatting Rust code..."
    cargo fmt --all
    @echo "  → Running clippy linting..."
    cargo clippy --all-targets -- -D warnings
    @echo "  → Checking UI code formatting..."
    cd ui/desktop && pnpm run lint:check
    @echo ""
    @echo "✅ All style checks passed!"

test-buzz:
    node --test buzz/*.test.mjs
    for file in buzz/create_github_manager buzz/create_issue_channel buzz/list_issue_work buzz/syncissues buzz/github_manager.mjs; do node --check "$file"; done

# Default release command
release-binary:
    @echo "Building release version..."
    cargo build --release -p goose-cli --bin goose
    @just copy-binary

# Build Windows executable on a Windows host
[unix]
release-windows:
    @echo "just release-windows requires a Windows host because Goose Windows releases build the MSVC target. Use .github/workflows/bundle-windows.yml for CI builds."
    @exit 1

[windows]
release-windows:
    @powershell.exe -NoProfile -ExecutionPolicy Bypass -Command 'rustup target add x86_64-pc-windows-msvc; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }; cargo build --release --target x86_64-pc-windows-msvc -p goose-cli --bin goose; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }; Write-Host "Windows executable created at ./target/x86_64-pc-windows-msvc/release/goose.exe"'

# Build for Intel Mac
release-intel:
    @echo "Building release version for Intel Mac..."
    cargo build --release --target x86_64-apple-darwin
    @just copy-binary-intel

copy-binary BUILD_MODE="release":
    @rm -f ./ui/desktop/src/bin/goosed
    @if [ -f ./target/{{BUILD_MODE}}/goose ]; then \
        echo "Copying goose CLI binary from target/{{BUILD_MODE}}..."; \
        rm -f ./ui/desktop/src/bin/goose; \
        cp -p ./target/{{BUILD_MODE}}/goose ./ui/desktop/src/bin/; \
    else \
        echo "goose CLI binary not found in target/{{BUILD_MODE}}"; \
        exit 1; \
    fi

# Copy binary command for Intel build
copy-binary-intel:
    @rm -f ./ui/desktop/src/bin/goosed
    @if [ -f ./target/x86_64-apple-darwin/release/goose ]; then \
        echo "Copying Intel goose CLI binary to ui/desktop/src/bin..."; \
        rm -f ./ui/desktop/src/bin/goose; \
        cp -p ./target/x86_64-apple-darwin/release/goose ./ui/desktop/src/bin/; \
    else \
        echo "Intel goose CLI binary not found."; \
        exit 1; \
    fi

# Copy Windows binary command on a Windows host
[unix]
copy-binary-windows:
    @echo "just copy-binary-windows requires a Windows host because it copies the MSVC build output."
    @exit 1

[windows]
copy-binary-windows:
    @powershell.exe -NoProfile -ExecutionPolicy Bypass -Command 'if (Test-Path ./target/x86_64-pc-windows-msvc/release/goose.exe) { \
        Write-Host "Copying Windows binary to ui/desktop/src/bin..."; \
        New-Item -ItemType Directory -Force "./ui/desktop/src/bin" | Out-Null; \
        Remove-Item -Path "./ui/desktop/src/bin/goosed.exe" -Force -ErrorAction SilentlyContinue; \
        Copy-Item -Path "./target/x86_64-pc-windows-msvc/release/goose.exe" -Destination "./ui/desktop/src/bin/" -Force; \
    } else { \
        Write-Host "Windows binary not found." -ForegroundColor Red; \
        exit 1; \
    }'

# Run UI with latest
run-ui:
    @just release-binary
    @echo "Running UI..."
    cd ui/desktop && pnpm install && pnpm run start-gui

run-ui-playwright:
    #!/usr/bin/env sh
    just release-binary
    echo "Running UI with Playwright debugging..."
    RUN_DIR="$HOME/goose-runs/$(date +%Y%m%d-%H%M%S)"
    mkdir -p "$RUN_DIR"
    echo "Using isolated directory: $RUN_DIR"
    cd ui/desktop && ENABLE_PLAYWRIGHT=true GOOSE_PATH_ROOT="$RUN_DIR" pnpm run start-gui

run-ui-only:
    @echo "Running UI..."
    cd ui/desktop && pnpm install && pnpm run start-gui

debug-ui:
    @echo "🚀 Starting goose frontend in external ACP backend mode"
    cd ui/desktop && \
    export GOOSE_EXTERNAL_BACKEND=true && \
    export GOOSE_SERVER__SECRET_KEY="${GOOSE_SERVER__SECRET_KEY:-test}" && \
    pnpm install && \
    pnpm run start-gui

# Run UI with main process debugging enabled
# To debug main process:
# 1. Run: just debug-ui-main-process
# 2. Open Chrome → chrome://inspect
# 3. Click "Open dedicated DevTools for Node"
# 4. If not auto-detected, click "Configure" and add: localhost:9229

debug-ui-main-process:
	@echo "🔍 Starting goose UI with main process debugging enabled"
	@just release-binary
	cd ui/desktop && \
	pnpm install && \
	pnpm run start-gui-debug

# Package the desktop app locally for testing (macOS)
# Applies ad-hoc code signing with entitlements (needed for mic access, etc.)
package-ui:
    @just release-binary
    @echo "Packaging desktop app..."
    cd ui/desktop && pnpm install && pnpm run package
    @echo "Signing with entitlements..."
    codesign --force --deep --sign - --entitlements ui/desktop/entitlements.plist ui/desktop/out/Goose-darwin-arm64/Goose.app
    @echo "Done! Launch with: open ui/desktop/out/Goose-darwin-arm64/Goose.app"

# Run UI with latest (Windows version)
run-ui-windows:
    @just release-windows
    @powershell.exe -Command "Write-Host 'Copying Windows binary...'"
    @just copy-binary-windows
    @powershell.exe -Command "Write-Host 'Running UI...'; Set-Location ui/desktop; pnpm install; pnpm run start-gui"

# Run Docusaurus server for documentation
run-docs:
    @echo "Running docs server..."
    cd documentation && yarn && yarn start

# Run server
run-server:
    @echo "Running external ACP backend..."
    GOOSE_SERVER__SECRET_KEY="${GOOSE_SERVER__SECRET_KEY:-test}" cargo run -p goose-cli --bin goose -- serve --platform desktop --enable-scheduler --host 127.0.0.1 --port 3000

# Check if checked-in ACP artifacts are up-to-date and the docs can be rendered
check-acp-artifacts: generate-acp-types generate-acp-docs
    #!/usr/bin/env bash
    set -e
    echo "🔍 Checking generated ACP artifacts are up-to-date..."
    if ! git diff --exit-code crates/goose/acp-schema.json crates/goose/acp-meta.json ui/goose-acp-client/src/generated/; then
      echo ""
      echo "❌ ACP generated files are out of date!"
      echo ""
      echo "Run 'just generate-acp-types' locally, then commit the changes."
      exit 1
    fi
    echo "✅ Generated ACP artifacts are up-to-date"

# Generate ACP JSON schema from Rust types
generate-acp-schema:
    @echo "Generating ACP schema..."
    cd crates/goose && cargo run --features code-mode,local-inference,aws-providers,telemetry,otel,rustls-tls,system-keyring --bin generate-acp-schema
    @echo "ACP schema generated: crates/goose/acp-schema.json, crates/goose/acp-meta.json"

# Generate ACP TypeScript types from JSON schema (requires generate-acp-schema first)
generate-acp-types: generate-acp-schema
    @echo "Generating ACP TypeScript types..."
    cd ui/goose-acp-client && npx tsx generate-schema.ts
    @echo "ACP TypeScript types generated for the ACP client package."

# Generate ACP documentation from the existing JSON schema and metadata
generate-acp-docs:
    @echo "Generating ACP documentation..."
    node documentation/scripts/generate-acp-docs.js
    @echo "ACP documentation generated for the docs build."

# Build ACP client TypeScript package (schema + types + compile)
build-acp-client: generate-acp-types
    @echo "Compiling ACP TypeScript..."
    cd ui/goose-acp-client && pnpm run build:ts
    @echo "ACP client package built."

# Generate manpages for the CLI
generate-manpages:
    @echo "Generating manpages..."
    cargo run -p goose-cli --bin generate_manpages
    @echo "Manpages generated at target/man/"

# make GUI with latest binary
lint-ui:
    cd ui/desktop && pnpm run lint:check

# make GUI with latest binary
make-ui:
    @just release-binary
    cd ui/desktop && pnpm run bundle:default

# make GUI with latest Windows binary on a Windows host
[unix]
make-ui-windows:
    @echo "just make-ui-windows requires a Windows host because Goose Windows releases build the MSVC target. Use .github/workflows/bundle-windows.yml for CI builds."
    @exit 1

[windows]
make-ui-windows:
    @just release-windows
    @just copy-binary-windows
    @powershell.exe -NoProfile -ExecutionPolicy Bypass -Command 'Set-Location ui/desktop; $env:ELECTRON_PLATFORM="win32"; node scripts/prepare-platform-binaries.js; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }; pnpm run make --platform=win32 --arch=x64; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }; Write-Host "Windows package build complete!"'

# make GUI with latest binary
make-ui-intel:
    @just release-intel
    cd ui/desktop && pnpm run bundle:intel



# Run UI with debug build
run-dev:
    @echo "Building development version..."
    cargo build
    @just copy-binary debug
    @echo "Running UI..."
    cd ui/desktop && pnpm run start-gui

# Install all dependencies (run once after fresh clone)
install-deps:
    cd ui/desktop && pnpm install
    cd documentation && yarn

ensure-release-branch:
    #!/usr/bin/env bash
    branch=$(git rev-parse --abbrev-ref HEAD); \
    if [[ ! "$branch" == release/* ]]; then \
        echo "Error: You are not on a release branch (current: $branch)"; \
        exit 1; \
    fi

    # check that main is up to date with upstream main
    git fetch
    # @{u} refers to upstream branch of current branch
    if [ "$(git rev-parse HEAD)" != "$(git rev-parse @{u})" ]; then \
        echo "Error: Your branch is not up to date with the upstream branch"; \
        echo "  ensure your branch is up to date (git pull)"; \
        exit 1; \
    fi

# validate the version is semver, and not the current version
validate version:
    #!/usr/bin/env bash
    if [[ ! "{{ version }}" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-.*)?$ ]]; then
      echo "[error]: invalid version '{{ version }}'."
      echo "  expected: semver format major.minor.patch or major.minor.patch-<suffix>"
      exit 1
    fi

    current_version=$(just get-tag-version)
    if [[ "{{ version }}" == "$current_version" ]]; then
      echo "[error]: current_version '$current_version' is the same as target version '{{ version }}'"
      echo "  expected: new version in semver format"
      exit 1
    fi

get-next-minor-version:
    @python -c "import sys; v=sys.argv[1].split('.'); print(f'{v[0]}.{int(v[1])+1}.0')" $(just get-tag-version)

get-next-patch-version:
    @python -c "import sys; v=sys.argv[1].split('.'); print(f'{v[0]}.{v[1]}.{int(v[2])+1}')" $(just get-tag-version)

# derive the prior release tag from a version
# patch bump (e.g. 1.25.1): prior is v1.25.0 (deterministic)
# minor bump (e.g. 1.26.0): prior is highest v1.25.* GitHub release
get-prior-version version:
    #!/usr/bin/env bash
    IFS='.' read -r major minor patch <<< "{{ version }}"
    if [[ "$patch" -gt 0 ]]; then
      echo "v${major}.${minor}.$((patch - 1))"
    elif [[ "$minor" -gt 0 ]]; then
      prev_minor=$((minor - 1))
      prefix="v${major}.${prev_minor}."
      best=$(gh release list --limit 100 --exclude-drafts --exclude-pre-releases \
        --json tagName --jq "[.[] | select(.tagName | startswith(\"${prefix}\"))][0].tagName")
      if [[ -n "$best" && "$best" != "null" ]]; then
        echo "$best"
      fi
    fi

# update version numbers in all manifests
bump-version version:
    @just validate {{ version }} || exit 1
    @uvx --from=toml-cli toml set --toml-path=Cargo.toml "workspace.package.version" {{ version }}
    @cd ui/desktop && npm pkg set "version={{ version }}"
    @node ui/scripts/npm-versions.mjs set {{ version }}
    # update Cargo.lock after bumping versions in Cargo.toml
    @cargo update --workspace
    @just check-npm-versions

check-npm-versions:
    @node ui/scripts/npm-versions.mjs check

# rebuild canonical model registry and mapping report from models.dev
build-canonical-models:
    @cargo run --bin build_canonical_models

# bump version, rebuild canonical models, and commit
prepare-release version:
    @just bump-version {{ version }}
    @just build-canonical-models
    @git add \
        Cargo.toml \
        Cargo.lock \
        ui/desktop/package.json \
        ui/goose-acp-client/package.json \
        ui/goose-acp/package.json \
        ui/goose-binary/*/package.json \
        ui/pnpm-lock.yaml \
        crates/goose-provider-types/src/canonical/data/canonical_models.json \
        crates/goose-provider-types/src/canonical/data/provider_metadata.json
    @git commit --message "chore(release): release version {{ version }}"

# extract version from Cargo.toml
get-tag-version:
    @uvx --from=toml-cli toml get --toml-path=Cargo.toml "workspace.package.version"

# create the git tag from Cargo.toml, checking we're on a release branch
tag: ensure-release-branch
    git tag v$(just get-tag-version)

# create tag and push to origin (use this when release branch is merged to main)
tag-push: tag
    # this will kick of ci for release
    git push origin tag v$(just get-tag-version)

# generate release notes from git commits
release-notes old:
    #!/usr/bin/env bash
    git log --pretty=format:"- %s" {{ old }}..v$(just get-tag-version)

### s = file separator based on OS
s := if os() == "windows" { "\\" } else { "/" }
linux_vulkan_features := if os() == "linux" { "--features vulkan" } else { "" }

### testing/debugging
os:
  echo "{{os()}}"
  echo "{{s}}"

# Make just work on Window
set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]

### Build the core code
### profile = --release or "" for debug
### allparam = OR/AND/ANY/NONE --workspace --all-features --all-targets
win-bld profile allparam:
  cargo build {{profile}} {{allparam}}

### Build just debug
win-bld-dbg:
  just win-bld " " " "

### Build debug and test, examples,...
win-bld-dbg-all:
  just win-bld " " "--workspace --all-targets --all-features"

### Build just release
win-bld-rls:
  just win-bld "--release" " "

### Build release and test, examples, ...
win-bld-rls-all:
  just win-bld "--release" "--workspace --all-targets --all-features"

### Install pnpm stuff
win-app-deps:
  cd ui{{s}}desktop ; pnpm install

### Windows copy {release|debug} files to ui\desktop\src\bin
### s = os dependent file separator
### profile = release or debug
win-copy-win profile:
  copy target{{s}}{{profile}}{{s}}*.exe ui{{s}}desktop{{s}}src{{s}}bin
  copy target{{s}}{{profile}}{{s}}*.dll ui{{s}}desktop{{s}}src{{s}}bin
  if exist ui{{s}}desktop{{s}}src{{s}}bin{{s}}goosed.exe del /f /q ui{{s}}desktop{{s}}src{{s}}bin{{s}}goosed.exe

### "Other" copy {release|debug} files to ui/desktop/src/bin
### s = os dependent file separator
### profile = release or debug
win-copy-oth profile:
  find target{{s}}{{profile}}{{s}} -maxdepth 1 -type f -executable -print -exec cp {} ui{{s}}desktop{{s}}src{{s}}bin \;

### copy files depending on OS
### profile = release or debug
win-app-copy profile="release":
  just win-copy-{{ if os() == "windows" { "win" } else { "oth" } }} {{profile}}

### Only copy binaries, pnpm install, start-gui
### profile = release or debug
### s = os dependent file separator
win-app-run profile:
  just win-app-copy {{profile}}
  just win-app-deps
  cd ui{{s}}desktop ; pnpm run start-gui

### Only run debug desktop, no build
win-run-dbg:
  just win-app-run "debug"

### Only run release desktop, nu build
win-run-rls:
  just win-app-run "release"

### Build and run debug desktop. tot = cli and desktop
### allparam = nothing or -all passed on command line
### -all = build with --workspace --all-targets --all-features
win-total-dbg *allparam:
  just win-bld-dbg{{allparam}}
  just win-run-dbg

### Build and run release desktop
### allparam = nothing or -all passed on command line
### -all = build with --workspace --all-targets --all-features
win-total-rls *allparam:
  just win-bld-rls{{allparam}}
  just win-run-rls

# Build the binaries the MCP conformance driver needs.
mcp-conformance-build:
  cargo build -p goose-cli --bin goose --bin mcp_conformance_driver

# suite: all, core, extensions, backcompat, auth, metadata, draft, sep-835
# build: "false" reuses the existing target/debug binaries instead of rebuilding
# Example: just mcp-conformance
# Example: just mcp-conformance 2025-11-25 auth
# Example: just mcp-conformance 2025-11-25 auth 0.2.0-alpha.10
# Example: just mcp-conformance 2025-11-25 auth 0.2.0-alpha.10 false
# Example: just mcp-conformance 2025-11-25 all 0.2.0-alpha.10 true crates/goose-cli/tests/mcp-conformance/expected-failures-2025-11-25-0.2.0-alpha.10.yaml
[doc("Run an MCP client conformance suite against Goose.")]
mcp-conformance version="2025-11-25" suite="all" conformance_version="0.2.0-alpha.10" build="true" baseline="":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ "{{build}}" = "true" ]; then
    just mcp-conformance-build
  elif [ ! -x target/debug/mcp_conformance_driver ]; then
    echo "target/debug/mcp_conformance_driver not found; run 'just mcp-conformance-build' first" >&2
    exit 1
  fi
  baseline_args=()
  if [ -n "{{baseline}}" ]; then
    baseline_args=(--expected-failures "{{baseline}}")
  fi
  GOOSE_DISABLE_KEYRING=1 npx -y @modelcontextprotocol/conformance@{{conformance_version}} client --command "target/debug/mcp_conformance_driver" --spec-version "{{version}}" --suite "{{suite}}" ${baseline_args[@]+"${baseline_args[@]}"}

build-test-tools:
  cargo build -p goose-test

record-mcp-tests: build-test-tools
  GOOSE_RECORD_MCP=1 cargo test --package goose --test mcp_integration_test
  git add crates/goose/tests/mcp_replays/
