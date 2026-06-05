#!/usr/bin/env bash
# ABOUTME: Local pre-push gate for the runner/ Rust workspace — fmt, architectural
# ABOUTME: validation, clippy (deny warnings), tests; stamps .git/validation-passed on success.
#
# Tiers, run in order; the first failure aborts:
#   Tier 0  cargo fmt --all -- --check
#   Tier 1  architectural-validation.sh (forbidden patterns / file size)
#   Tier 2  cargo clippy --all-targets --all-features -- -D warnings
#   Tier 3  cargo test --all-targets
#
# On full success it writes a unix timestamp to <git-dir>/validation-passed.
# The pre-push hook treats that marker as valid for VALIDATION_TTL_SECONDS
# (15 minutes); a stale or missing marker re-requires this script.
#
# Runnable from anywhere inside the repo: it cd's into runner/ itself.

set -euo pipefail

# 15-minute marker TTL — kept in sync with .githooks/pre-push.
VALIDATION_TTL_SECONDS=900

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# scripts/ci/ -> scripts/ -> runner/
RUNNER_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$RUNNER_DIR"

echo "==> Tier 0: cargo fmt --all -- --check"
cargo fmt --all -- --check

echo "==> Tier 1: architectural-validation.sh"
"$SCRIPT_DIR/architectural-validation.sh"

echo "==> Tier 2: cargo clippy --all-targets --all-features -- -D warnings"
cargo clippy --all-targets --all-features -- -D warnings

echo "==> Tier 3: cargo test --all-targets"
cargo test --all-targets

GIT_DIR="$(git rev-parse --git-dir)"
date +%s > "$GIT_DIR/validation-passed"

echo ""
echo "pre-push-validate: ALL TIERS PASSED"
echo "Stamped $GIT_DIR/validation-passed (valid for $((VALIDATION_TTL_SECONDS / 60)) minutes)."
exit 0
