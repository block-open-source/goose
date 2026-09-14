# Native Binary Packages for goose

This directory contains the npm package scaffolding for distributing the
`goose` Rust binary as platform-specific npm packages.

## Packages

| Package | Platform |
|---------|----------|
| `@aaif/goose-binary-darwin-arm64` | macOS Apple Silicon |
| `@aaif/goose-binary-darwin-x64` | macOS Intel |
| `@aaif/goose-binary-linux-arm64` | Linux ARM64 |
| `@aaif/goose-binary-linux-x64` | Linux x64 |
| `@aaif/goose-binary-win32-x64` | Windows x64 |

## Usage

These are platform-specific implementation dependencies and are not intended
to be installed directly. Install `@aaif/goose-acp` instead. It installs the
appropriate package automatically and provides the `goose` command. Each
binary package contains its native executable. Its platform-specific internal
command preserves executable permissions during npm packing;
`@aaif/goose-acp` remains the sole owner of the supported `goose` command.

## Release preparation

The `.github/workflows/publish-npm.yml` workflow downloads the binaries from an
exact versioned Goose release and prepares the platform package tarballs.
By default it only uploads the verified tarballs as a workflow artifact. Set
the manual `publish` input to publish them through the protected npm production
environment.
