#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "Usage: $0 PACKAGE_DIRECTORY RELEASE_VERSION" >&2
  exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
package_dir="$1"
release_version="$2"
packages=(
  @aaif/goose-binary-darwin-arm64
  @aaif/goose-binary-darwin-x64
  @aaif/goose-binary-linux-arm64
  @aaif/goose-binary-linux-x64
  @aaif/goose-binary-win32-x64
  @aaif/goose-acp
  @aaif/goose-acp-client
)

calculate_integrity() {
  "$repo_root/bin/node" -e '
    const { createHash } = require("node:crypto");
    const { readFileSync } = require("node:fs");
    const digest = createHash("sha512")
      .update(readFileSync(process.argv[1]))
      .digest("base64");
    console.log(`sha512-${digest}`);
  ' "$1"
}

publish_package() {
  local package_name="$1"
  local tarball="$package_dir/aaif-${package_name#@aaif/}-$release_version.tgz"
  local package_spec="$package_name@$release_version"
  local published_integrity
  local local_integrity
  local view_output

  test -f "$tarball"
  local_integrity="$(calculate_integrity "$tarball")"

  if view_output=$("$repo_root/bin/npm" view \
    "$package_spec" dist.integrity 2>&1); then
    published_integrity="$view_output"
    if [[ "$published_integrity" != "$local_integrity" ]]; then
      echo "$package_spec is already published with different contents" >&2
      exit 1
    fi

    echo "$package_spec is already published with matching contents"
    return
  fi

  if [[ "$view_output" != *"E404"* ]]; then
    echo "$view_output" >&2
    return 1
  fi

  "$repo_root/bin/npm" publish "$tarball" --access public --provenance
}

(
  cd "$package_dir"
  sha256sum --check SHA256SUMS
)

for package_name in "${packages[@]}"; do
  publish_package "$package_name"
done
