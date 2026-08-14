#!/usr/bin/env bash
# Check every media file's sidecar checksum against the file itself, and flag
# any media file that has no sidecar at all. Plain bash, no `legacy` install
# required -- only needs sha256sum and (if present) a YAML-ish grep of the
# sidecar's "sha256:" line.
#
# Usage: refresh-media.sh [archive-root]

set -euo pipefail

archive="${1:-.}"
cd "$archive"

if [ ! -d media ]; then
    echo "error: no media/ directory under $archive" >&2
    exit 2
fi

status=0
checked=0

while IFS= read -r -d '' file; do
    sidecar="${file}.yaml"
    checked=$((checked + 1))
    if [ ! -f "$sidecar" ]; then
        echo "MISSING SIDECAR: $file" >&2
        status=1
        continue
    fi
    recorded=$(grep -m1 '^sha256:' "$sidecar" | sed 's/^sha256:[[:space:]]*//')
    actual=$(sha256sum "$file" | awk '{print $1}')
    if [ "$recorded" != "$actual" ]; then
        echo "CHECKSUM MISMATCH: $file (sidecar says $recorded, actual is $actual)" >&2
        status=1
    fi
done < <(find media -type f ! -name '*.yaml' -print0)

echo "Checked $checked media file(s)."
exit "$status"
