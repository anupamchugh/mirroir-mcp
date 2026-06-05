#!/usr/bin/env bash
# ABOUTME: Greps runner/src for release-blocking forbidden patterns (anyhow macro,
# ABOUTME: anyhow::Result/Context, unwrap/expect/panic in prod, placeholders, stray allows, >500-line files).
#
# Exits non-zero on the first category that has a hit. The current tree is
# expected to pass clean (exit 0). Intended to run from anywhere inside the
# repo; it resolves runner/src relative to its own location.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# scripts/ci/ -> scripts/ -> runner/
RUNNER_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
SRC_DIR="$RUNNER_DIR/src"

if [ ! -d "$SRC_DIR" ]; then
    echo "ERROR: source directory not found: $SRC_DIR"
    exit 1
fi

violations=0

report() {
    # report <category> <details...>
    echo "FORBIDDEN: $1"
    shift
    printf '%s\n' "$@" | sed 's/^/  /'
    echo ""
    violations=$((violations + 1))
}

# List of all production .rs files.
mapfile -t rs_files < <(find "$SRC_DIR" -name '*.rs' | sort)

# ---------------------------------------------------------------------------
# Helper: emit only the production portion of a file (everything before the
# first `#[cfg(test)]` line) with full-line comments stripped, so prose that
# merely mentions a forbidden token in a doc comment does not false-positive.
# ---------------------------------------------------------------------------
production_lines() {
    # production_lines <file> -> stdout: "lineno:content"
    awk '
        /#\[cfg\(test\)\]/ { exit }
        {
            line = $0
            stripped = line
            sub(/^[[:space:]]+/, "", stripped)
            # Skip full-line comments (// , /// , //! , leading * of block comments).
            if (stripped ~ /^\/\// || stripped ~ /^\*/) next
            print NR ":" line
        }
    ' "$1"
}

# ---------------------------------------------------------------------------
# 1. anyhow!() / anyhow::anyhow!() macro
# ---------------------------------------------------------------------------
hits=""
for f in "${rs_files[@]}"; do
    found="$(production_lines "$f" | grep -nE 'anyhow!|anyhow::anyhow!' || true)"
    [ -n "$found" ] && hits="$hits\n$f:$found"
done
[ -n "$hits" ] && report "anyhow!() macro (use a structured RunnerError variant)" "$(echo -e "$hits")"

# ---------------------------------------------------------------------------
# 2. anyhow::Result type alias
# ---------------------------------------------------------------------------
hits=""
for f in "${rs_files[@]}"; do
    found="$(production_lines "$f" | grep -nE 'anyhow::Result' || true)"
    [ -n "$found" ] && hits="$hits\n$f:$found"
done
[ -n "$hits" ] && report "anyhow::Result (use crate::error::Result)" "$(echo -e "$hits")"

# ---------------------------------------------------------------------------
# 3. anyhow::Context
# ---------------------------------------------------------------------------
hits=""
for f in "${rs_files[@]}"; do
    found="$(production_lines "$f" | grep -nE 'anyhow::Context' || true)"
    [ -n "$found" ] && hits="$hits\n$f:$found"
done
[ -n "$hits" ] && report "anyhow::Context (add a structured RunnerError variant)" "$(echo -e "$hits")"

# ---------------------------------------------------------------------------
# 4. .unwrap() / .expect( / panic! / unimplemented! / todo! in NON-test code
#    (only the production portion of each file, comments stripped)
# ---------------------------------------------------------------------------
hits=""
for f in "${rs_files[@]}"; do
    found="$(production_lines "$f" | grep -nE '\.unwrap\(\)|\.expect\(|panic!|unimplemented!|todo!' || true)"
    [ -n "$found" ] && hits="$hits\n$f:$found"
done
[ -n "$hits" ] && report ".unwrap()/.expect(/panic!/unimplemented!/todo! in production code" "$(echo -e "$hits")"

# ---------------------------------------------------------------------------
# 5. Placeholder / stub language (whole file, case-insensitive).
#    "placeholder" matches only in stub contexts (placeholder implementation /
#    placeholder code / TODO: placeholder), NOT the noun used to describe
#    legitimate `${VAR}` substitution in doc comments.
# ---------------------------------------------------------------------------
hits=""
for f in "${rs_files[@]}"; do
    found="$(grep -niE 'Implementation would|TODO: Implementation|stub implementation|placeholder (implementation|code)|TODO:? placeholder|In future versions|Will implement' "$f" || true)"
    [ -n "$found" ] && hits="$hits\n$f:$found"
done
[ -n "$hits" ] && report "placeholder/stub language" "$(echo -e "$hits")"

# ---------------------------------------------------------------------------
# 6. #[allow(clippy::...)] outside the approved list
# ---------------------------------------------------------------------------
APPROVED="cast_possible_truncation cast_sign_loss cast_precision_loss \
missing_const_for_fn struct_excessive_bools too_many_lines \
significant_drop_tightening module_name_repetitions let_unit_value \
option_if_let_else bool_to_int_with_if type_complexity too_many_arguments use_self \
cast_possible_wrap cognitive_complexity"

hits=""
for f in "${rs_files[@]}"; do
    # Each allow occurrence: extract the lint names after clippy::
    while IFS= read -r line; do
        [ -z "$line" ] && continue
        lints="$(echo "$line" | grep -oE 'clippy::[a-z_]+' | sed 's/clippy:://')"
        for lint in $lints; do
            case " $APPROVED " in
                *" $lint "*) ;;  # approved
                *) hits="$hits\n$f: $line" ;;
            esac
        done
    done < <(grep -nE '#\[allow\(clippy::' "$f" || true)
done
[ -n "$hits" ] && report "#[allow(clippy::...)] outside the approved list" "$(echo -e "$hits")"

# ---------------------------------------------------------------------------
# 7. Files exceeding 500 lines
# ---------------------------------------------------------------------------
hits=""
for f in "${rs_files[@]}"; do
    n="$(wc -l < "$f" | tr -d ' ')"
    if [ "$n" -gt 500 ]; then
        hits="$hits\n$f: $n lines (max 500)"
    fi
done
[ -n "$hits" ] && report "files exceeding 500 lines" "$(echo -e "$hits")"

# ---------------------------------------------------------------------------
if [ "$violations" -gt 0 ]; then
    echo "architectural-validation: FAILED ($violations forbidden category/ies)"
    exit 1
fi

echo "architectural-validation: OK (no forbidden patterns in $SRC_DIR)"
exit 0
