#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 auto|patch|minor|major|X.Y.Z" >&2
  exit 2
}

requested="${1:-auto}"
current="$(sed -n '/^\[package\]/,/^\[/ s/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"

if [[ ! "$current" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
  echo "Cargo.toml package version is not plain SemVer: $current" >&2
  exit 1
fi

head_text="$(git show HEAD:Cargo.toml 2>/dev/null || true)"
head_version="$(printf '%s\n' "$head_text" | sed -n '/^\[package\]/,/^\[/ s/^version = "\([^"]*\)"/\1/p' | head -1)"

if [[ "$requested" == "auto" && -n "$head_version" && "$current" != "$head_version" ]]; then
  ./scripts/check-version.sh
  echo "version already changed $head_version -> $current; automatic patch bump not needed"
  exit 0
fi

major="${BASH_REMATCH[1]}"
minor="${BASH_REMATCH[2]}"
patch="${BASH_REMATCH[3]}"

case "$requested" in
  auto|patch) next="$major.$minor.$((patch + 1))" ;;
  minor) next="$major.$((minor + 1)).0" ;;
  major) next="$((major + 1)).0.0" ;;
  *)
    if [[ "$requested" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
      next="$requested"
    else
      usage
    fi
    ;;
esac

if [[ "$next" == "$current" ]]; then
  echo "version is already $current" >&2
  exit 1
fi

python3 - "$current" "$next" <<'PY'
from pathlib import Path
import sys

old, new = sys.argv[1:]
path = Path("Cargo.toml")
text = path.read_text()
needle = f'version = "{old}"'
if text.count(needle) != 1:
    raise SystemExit(f"expected exactly one root package version {old!r} in Cargo.toml")
path.write_text(text.replace(needle, f'version = "{new}"', 1))
PY

cargo metadata --format-version 1 --no-deps >/dev/null
./scripts/check-version.sh

echo "bumped legacy $current -> $next"
echo "release tag for this version: v$next"
