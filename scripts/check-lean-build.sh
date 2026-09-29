#!/usr/bin/env bash
# Verifies REQ-404 (.specs/features/vector-engine/spec.md): a `lean` build
# must not pull `ort` or `instant-distance` into the dependency tree at all.
# Run from the repo root: bash scripts/check-lean-build.sh
set -euo pipefail

tree_output=$(cargo tree --no-default-features --features lean)

if echo "$tree_output" | grep -qE '^\S* (ort|instant-distance) v'; then
    echo "FAIL: lean build still pulls in ort or instant-distance:" >&2
    echo "$tree_output" | grep -E '^\S* (ort|instant-distance) v' >&2
    exit 1
fi

echo "OK: lean build excludes ort and instant-distance."
