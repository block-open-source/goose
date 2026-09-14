#!/usr/bin/env bash
# v8-goose downloads a prebuilt librusty_v8 archive into target/<profile>/gn_out
# and emits a rustc-link-search pointing at it. Cargo caches that build script
# output and replays it on later builds instead of re-running the script, but
# the Rust cache action prunes target/ before saving, so a restored cache can
# keep the replayed link-search while the 131MB archive is gone. Linking then
# fails with "could not find native static library `rusty_v8`".
#
# Drop the cached build script output whenever a directory it advertises is
# missing, which forces the script to re-run and download the archive again.
set -euo pipefail

[ -d target ] || exit 0

find target -type f -path '*/build/v8-goose-*/output' -print0 |
  while IFS= read -r -d '' output; do
    while IFS= read -r dir; do
      [ -n "$dir" ] || continue
      [ -d "$dir" ] && continue
      echo "v8-goose build output references missing $dir; forcing a rebuild"
      rm -rf "$(dirname "$output")"
      break
    done < <(sed -n 's/^cargo:rustc-link-search=\(.*\)$/\1/p' "$output")
  done
