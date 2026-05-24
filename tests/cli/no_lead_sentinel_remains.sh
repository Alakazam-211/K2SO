#!/usr/bin/env bash
# 0.39.0f Phase 2.1 final-final — sanity grep that the `__lead__`
# routing sentinel does NOT appear in production code paths.
#
# Background. Pre-0.37.0 K2SO addressed the workspace manager as
# `__lead__` everywhere — a string literal that doubled as agent name,
# DB column value, CLI fallback, and scheduler routing key. The
# 0.37.0 unification collapsed every workspace to a single primary
# agent whose identity is the workspace itself; the `__lead__` string
# became dead weight. This pass (0.39.0f Phase 2.1 final-final)
# removes the literal from every active code path.
#
# Tolerated occurrences (excluded by this grep):
#   - tests/                — assertions / fixture data
#   - drizzle_sql/0049_*    — the migration that rewrites the row data
#   - drizzle_sql/0042_*    — historical migration prose
#   - unification.rs        — pre-0.37.0 layout migration (legitimate
#                             migration helper that handles a directory
#                             named `__lead__` left behind on disk)
#   - documentation comments that explicitly describe the removal
#     (lines whose `__lead__` appears after `// 0.39.0f:`,
#      `// Post-0.39.0f:`, `// removed`, `// dropped`, etc.)
#
# Hard-fails when a literal `__lead__` sneaks back into a non-exempt
# production file.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$PROJECT_ROOT"

# Search active production code only: crates/, src-tauri/, cli/k2so.
# Exclude tests/, migration helpers, and the new sentinel migration.
HITS_FILE="$(mktemp -t k2so-no-lead-sentinel-XXXXXX)"
trap "rm -f '$HITS_FILE'" EXIT

# Grep, then filter. Each remaining hit is a failure.
# Walk every file matching `__lead__` and check each hit against the
# excluded categories:
#  - inside a `#[cfg(test)]` mod (test-only code)
#  - a `//` / `///` / `#` comment line
#  - a known migration / unification helper file
exclude_file() {
    local f="$1"
    case "$f" in
        crates/k2so-core/drizzle_sql/0049_drop_lead_sentinel_in_activity_feed.sql) return 0 ;;
        crates/k2so-core/drizzle_sql/0042_canonical_key_drop_agent_suffix.sql) return 0 ;;
        crates/k2so-core/src/agents/unification.rs) return 0 ;;
    esac
    return 1
}

# Pre-compute the line where each Rust file's `mod tests` block starts
# (if any). Hits at or after that line are inside the test mod.
test_mod_line() {
    local f="$1"
    grep -n '^[[:space:]]*\(mod tests\|#\[cfg(test)\][[:space:]]*$\)' "$f" 2>/dev/null \
        | head -1 \
        | awk -F: '{print $1}'
}

while IFS= read -r line; do
    # line shape: file:LN:content
    f="${line%%:*}"
    rest="${line#*:}"
    ln="${rest%%:*}"
    content="${rest#*:}"

    # File-level excludes.
    if exclude_file "$f"; then continue; fi

    # Comment lines (// or ///).
    case "$content" in
        ''|*[![:space:]]*) ;;
    esac
    trimmed="${content#"${content%%[![:space:]]*}"}"
    case "$trimmed" in
        //*|///*|\#*) continue ;;
    esac

    # Test mod: skip if `ln` >= the first `mod tests` / `#[cfg(test)]` line.
    if [[ "$f" == *.rs ]]; then
        tm_line="$(test_mod_line "$f")"
        if [[ -n "$tm_line" && "$ln" -ge "$tm_line" ]]; then
            continue
        fi
    fi

    # Specific exemptions for inline comments and one-off cases.
    case "$line" in
        *session_events.rs*pane_group_id_from_agent*) continue ;;
    esac

    echo "$line"
done < <(grep -rn '__lead__' crates/ src-tauri/ cli/k2so 2>/dev/null) > "$HITS_FILE"

if [ -s "$HITS_FILE" ]; then
    echo "FAIL: production code still contains \`__lead__\` literal:" >&2
    echo "" >&2
    cat "$HITS_FILE" >&2
    echo "" >&2
    echo "If any of these are intentional (e.g., a new migration helper" >&2
    echo "or a comment documenting the removal), update the exclusion" >&2
    echo "list in tests/cli/no_lead_sentinel_remains.sh." >&2
    exit 1
fi

echo "OK: no \`__lead__\` routing sentinel remains in production code"
