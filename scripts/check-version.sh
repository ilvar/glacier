#!/usr/bin/env bash
set -euo pipefail

cargo_version="$(sed -n '/^\[package\]/,/^\[/ s/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"
lock_version="$(awk '
  $0 == "[[package]]" { in_package = 1; name = ""; next }
  in_package && /^name = "legacy"$/ { name = "legacy"; next }
  in_package && name == "legacy" && /^version = / {
    gsub(/^version = "|"$/, ""); print; exit
  }
' Cargo.lock)"

if [[ -z "$cargo_version" || -z "$lock_version" ]]; then
  echo "could not read legacy version from Cargo.toml/Cargo.lock" >&2
  exit 1
fi

if [[ "$cargo_version" != "$lock_version" ]]; then
  echo "version mismatch: Cargo.toml=$cargo_version Cargo.lock=$lock_version" >&2
  echo "run: cargo metadata --format-version 1 --no-deps" >&2
  exit 1
fi

echo "legacy version $cargo_version"
