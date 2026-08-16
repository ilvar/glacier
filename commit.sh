#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || -z "$1" ]]; then
  echo "usage: ./commit.sh \"commit message\"" >&2
  exit 2
fi

if [[ -z "$(git status --porcelain)" ]]; then
  echo "nothing to commit" >&2
  exit 1
fi

if ! command -v pre-commit >/dev/null 2>&1; then
  echo "pre-commit is required; install it from https://pre-commit.com/" >&2
  exit 1
fi

./scripts/bump-version.sh auto
pre-commit run --all-files
git diff --check
git add -A

if git diff --cached --quiet; then
  echo "nothing to commit after checks" >&2
  exit 1
fi

# Hooks were just run against the complete working tree above. Avoid running the
# expensive Rust suite twice while still making direct `git commit` use hooks.
git commit --no-verify -m "$1"
