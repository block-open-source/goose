#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "Usage: $0 RELEASE_TAG OUTPUT_DIRECTORY" >&2
  exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
release_tag="$1"
release_version="${release_tag#v}"
output_dir="$2"
repository="${GITHUB_REPOSITORY:-aaif-goose/goose}"
work_dir="$(mktemp -d)"
asset_dir="$work_dir/assets"
extract_root="$work_dir/extracted"
wrapper_extract_dir="$work_dir/wrapper"
smoke_dir="$work_dir/smoke"
trap 'rm -rf "$work_dir"' EXIT
mkdir -p "$asset_dir" "$wrapper_extract_dir" "$smoke_dir"

download_release_binaries() {
  gh release download "$release_tag" \
    --repo "$repository" \
    --dir "$asset_dir" \
    --pattern 'goose-aarch64-apple-darwin.tar.bz2' \
    --pattern 'goose-x86_64-apple-darwin.tar.bz2' \
    --pattern 'goose-aarch64-unknown-linux-gnu.tar.bz2' \
    --pattern 'goose-x86_64-unknown-linux-gnu.tar.bz2' \
    --pattern 'goose-x86_64-pc-windows-msvc.zip'
}

copy_unix_binary() {
  local platform="$1"
  local target="$2"
  local extract_dir="$extract_root/$platform"
  local destination="$repo_root/ui/goose-binary/goose-binary-$platform/bin/goose"

  mkdir -p "$extract_dir" "$(dirname "$destination")"
  tar -xjf "$asset_dir/goose-$target.tar.bz2" -C "$extract_dir"
  test -f "$extract_dir/goose"
  rm -f "$destination"
  install -m 755 "$extract_dir/goose" "$destination"
}

copy_release_binaries() {
  copy_unix_binary darwin-arm64 aarch64-apple-darwin
  copy_unix_binary darwin-x64 x86_64-apple-darwin
  copy_unix_binary linux-arm64 aarch64-unknown-linux-gnu
  copy_unix_binary linux-x64 x86_64-unknown-linux-gnu

  local extract_dir="$extract_root/win32-x64"
  local destination="$repo_root/ui/goose-binary/goose-binary-win32-x64/bin/goose.exe"
  mkdir -p "$extract_dir" "$(dirname "$destination")"
  unzip -q "$asset_dir/goose-x86_64-pc-windows-msvc.zip" -d "$extract_dir"
  test -f "$extract_dir/goose-package/goose.exe"
  rm -f "$destination"
  install -m 755 "$extract_dir/goose-package/goose.exe" "$destination"
}

assert_version() {
  local subject="$1"
  local actual_version="$2"

  if [[ "$actual_version" != "$release_version" ]]; then
    echo "$subject is $actual_version; expected $release_version" >&2
    exit 1
  fi
}

current_platform() {
  "$repo_root/bin/node" -p '`${process.platform}-${process.arch}`'
}

current_platform_binary() {
  local platform
  local executable="goose"
  platform="$(current_platform)"

  case "$platform" in
    darwin-arm64 | darwin-x64 | linux-arm64 | linux-x64) ;;
    win32-x64) executable="goose.exe" ;;
    *)
      echo "No Goose npm binary is available for $platform" >&2
      return 1
      ;;
  esac

  echo "$repo_root/ui/goose-binary/goose-binary-$platform/bin/$executable"
}

verify_release_versions() {
  local binary
  binary="$(current_platform_binary)"

  "$repo_root/bin/node" "$repo_root/ui/scripts/npm-versions.mjs" \
    check-release "$release_version"

  assert_version "Current-platform binary" \
    "$("$binary" --version | xargs)"
}

pack_packages() {
  local packages=(
    ui/goose-binary/goose-binary-darwin-arm64
    ui/goose-binary/goose-binary-darwin-x64
    ui/goose-binary/goose-binary-linux-arm64
    ui/goose-binary/goose-binary-linux-x64
    ui/goose-binary/goose-binary-win32-x64
    ui/goose-acp
    ui/goose-acp-client
  )
  local package

  mkdir -p "$output_dir"
  for package in "${packages[@]}"; do
    "$repo_root/bin/pnpm" --dir "$repo_root/$package" pack \
      --pack-destination "$output_dir"
  done

  tar -xzf "$output_dir/aaif-goose-acp-$release_version.tgz" \
    -C "$wrapper_extract_dir"
  "$repo_root/bin/node" "$repo_root/ui/scripts/npm-versions.mjs" \
    check-packed-wrapper "$wrapper_extract_dir/package/package.json"

  (
    cd "$output_dir"
    sha256sum -- *.tgz > SHA256SUMS
  )
}

verify_packed_wrapper() {
  local platform
  platform="$(current_platform)"

  (
    cd "$smoke_dir"
    "$repo_root/bin/npm" init --yes >/dev/null
    "$repo_root/bin/pnpm" add \
      "$output_dir/aaif-goose-binary-$platform-$release_version.tgz" \
      "$output_dir/aaif-goose-acp-$release_version.tgz"

    assert_version "Packed wrapper" \
      "$(env -u GOOSE_BINARY "$repo_root/bin/pnpm" exec goose --version | xargs)"
  )
}

echo "Preparing npm packages for $release_tag"

echo "Downloading release binaries"
download_release_binaries

echo "Copying binaries into platform packages"
copy_release_binaries

echo "Installing workspace dependencies"
"$repo_root/bin/pnpm" --dir "$repo_root/ui" install --frozen-lockfile

echo "Verifying release versions"
verify_release_versions

echo "Packing npm packages"
pack_packages

echo "Verifying the packed wrapper"
verify_packed_wrapper

echo "Prepared npm package tarballs in $output_dir"
