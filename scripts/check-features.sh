#!/usr/bin/env bash

set -euo pipefail

export BUILD_PROTO=1
export RUSTFLAGS="${RUSTFLAGS:+${RUSTFLAGS} }-D warnings"

cargo hack check \
    --locked \
    --workspace \
    --each-feature \
    --exclude-features default \
    --all-targets
