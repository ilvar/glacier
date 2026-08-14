#!/usr/bin/env bash
# Verify a legacy archive's integrity using only sha256sum and par2 -- no
# Python, no `legacy` binary required. Run from inside the archive root, or
# pass the archive root as the first argument.
#
# Usage: verify-archive.sh [archive-root]

set -euo pipefail

archive="${1:-.}"
cd "$archive"

status=0

if [ ! -f archive.yaml ]; then
    echo "error: $archive does not look like a legacy archive (no archive.yaml)" >&2
    exit 2
fi

if [ -f MANIFEST.sha256 ]; then
    echo "== Checking MANIFEST.sha256 =="
    if sha256sum -c MANIFEST.sha256; then
        echo "OK: all checksums match"
    else
        echo "FAIL: one or more files do not match MANIFEST.sha256" >&2
        status=1
    fi
else
    echo "warning: no MANIFEST.sha256 found, skipping checksum verification" >&2
fi

if [ -d sealed ]; then
    if ! command -v par2 >/dev/null 2>&1; then
        echo "warning: par2 is not installed, skipping recovery-data verification" >&2
    else
        echo "== Checking par2 recovery data for sealed/ =="
        for index in sealed/*.tar.age.par2; do
            [ -e "$index" ] || continue
            echo "-- $index --"
            if ! par2 verify -q "$index"; then
                echo "FAIL: $index did not verify" >&2
                status=1
            fi
        done
    fi
fi

exit "$status"
