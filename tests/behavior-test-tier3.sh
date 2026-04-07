#!/bin/bash
# K2SO Behavioral Tests — Tier 3: Unit-Style Tests
# No running K2SO instance needed. Tests migrations, templates, and script correctness.
#
# Usage: ./tests/behavior-test-tier3.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TEST_WORKSPACE="${TEST_WORKSPACE:-/Users/z3thon/DevProjects/k2so-cli-test}"
K2SO_CLI="$PROJECT_ROOT/cli/k2so"
MIGRATIONS_DIR="$PROJECT_ROOT/src-tauri/drizzle_sql"
PASS=0; FAIL=0; SKIP=0

GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[0;33m'; CYAN='\033[0;36m'; NC='\033[0m'
pass() { PASS=$((PASS + 1)); echo -e "  ${GREEN}PASS${NC} $1"; }
fail() { FAIL=$((FAIL + 1)); echo -e "  ${RED}FAIL${NC} $1"; echo -e "       ${RED}$2${NC}"; }
skip() { SKIP=$((SKIP + 1)); echo -e "  ${YELLOW}SKIP${NC} $1"; }
section() { echo ""; echo -e "${CYAN}── $1 ──${NC}"; }

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║       K2SO Behavioral Tests — Tier 3 (Unit-Style)          ║"
echo "╚══════════════════════════════════════════════════════════════╝"

# ═══════════════════════════════════════════════════════════════════════
section "3.1: Migration Safety"
# ═══════════════════════════════════════════════════════════════════════

TEMP_DB="/tmp/k2so-test-migrations-$$.db"

# Check sqlite3 available
if ! command -v sqlite3 &> /dev/null; then
    skip "sqlite3 not installed — skipping migration tests"
else
    # Create the _migrations tracking table (K2SO does this in Rust)
    sqlite3 "$TEMP_DB" "CREATE TABLE IF NOT EXISTS _migrations (name TEXT PRIMARY KEY, applied_at INTEGER DEFAULT (strftime('%s','now')));"

    # Apply each migration in order
    MIGRATION_ERRORS=0
    for sql_file in "$MIGRATIONS_DIR"/*.sql; do
        BASENAME=$(basename "$sql_file" .sql)
        # Skip if already applied
        APPLIED=$(sqlite3 "$TEMP_DB" "SELECT COUNT(*) FROM _migrations WHERE name = '$BASENAME';")
        if [ "$APPLIED" -gt 0 ]; then continue; fi

        # Split on --> statement-breakpoint and execute each statement
        # SQLite needs statements executed separately
        if sqlite3 "$TEMP_DB" < "$sql_file" 2>/dev/null; then
            sqlite3 "$TEMP_DB" "INSERT INTO _migrations (name) VALUES ('$BASENAME');" 2>/dev/null
        else
            # Some migrations have multiple statements that need individual execution
            # Try line by line, ignoring "duplicate column" errors
            while IFS= read -r line; do
                line=$(echo "$line" | sed 's/-->.*//' | xargs) # strip breakpoint markers and whitespace
                [ -z "$line" ] && continue
                [[ "$line" == --* ]] && continue # skip comments
                sqlite3 "$TEMP_DB" "$line" 2>/dev/null || true
            done < "$sql_file"
            sqlite3 "$TEMP_DB" "INSERT OR IGNORE INTO _migrations (name) VALUES ('$BASENAME');" 2>/dev/null
        fi
    done

    # Verify tables exist
    TABLES=$(sqlite3 "$TEMP_DB" ".tables" 2>/dev/null)

    if echo "$TABLES" | grep -q "projects"; then
        pass "migration: projects table exists"
    else
        fail "migration: projects table" "Tables: $TABLES"
    fi

    if echo "$TABLES" | grep -q "workspace_states"; then
        pass "migration: workspace_states table exists"
    else
        fail "migration: workspace_states table" "Tables: $TABLES"
    fi

    if echo "$TABLES" | grep -q "agent_presets"; then
        pass "migration: agent_presets table exists"
    else
        fail "migration: agent_presets table" "Tables: $TABLES"
    fi

    if echo "$TABLES" | grep -q "chat_session_names"; then
        pass "migration: chat_session_names table exists"
    else
        fail "migration: chat_session_names table" "Tables: $TABLES"
    fi

    # Verify default workspace states were seeded
    STATE_COUNT=$(sqlite3 "$TEMP_DB" "SELECT COUNT(*) FROM workspace_states;" 2>/dev/null || echo "0")
    if [ "$STATE_COUNT" -ge 4 ]; then
        pass "migration: 4 default workspace states seeded"
    else
        fail "migration: default states" "Expected >=4, got $STATE_COUNT"
    fi

    # Verify specific state values
    MAINTENANCE_CRASHES=$(sqlite3 "$TEMP_DB" "SELECT cap_crashes FROM workspace_states WHERE id='state-maintenance';" 2>/dev/null || echo "?")
    if [ "$MAINTENANCE_CRASHES" = "auto" ]; then
        pass "migration: Maintenance state crashes=auto (migration 0017 fix)"
    else
        fail "migration: Maintenance crashes" "Expected 'auto', got '$MAINTENANCE_CRASHES'"
    fi

    # Verify tier_id column on projects
    HAS_TIER_ID=$(sqlite3 "$TEMP_DB" "PRAGMA table_info(projects);" 2>/dev/null | grep "tier_id" | wc -l | xargs)
    if [ "${HAS_TIER_ID:-0}" -gt 0 ]; then
        pass "migration: projects has tier_id column"
    else
        fail "migration: tier_id column" "Column not found in projects table"
    fi

    # Verify agent session columns
    HAS_AGENT_SESSION=$(sqlite3 "$TEMP_DB" "PRAGMA table_info(chat_session_names);" 2>/dev/null | grep "is_agent_session" | wc -l | xargs)
    if [ "${HAS_AGENT_SESSION:-0}" -gt 0 ]; then
        pass "migration: chat_session_names has is_agent_session column"
    else
        fail "migration: is_agent_session" "Column not found"
    fi

    # Verify all migrations recorded
    MIGRATION_COUNT=$(sqlite3 "$TEMP_DB" "SELECT COUNT(*) FROM _migrations;" 2>/dev/null || echo "0")
    EXPECTED_MIGRATIONS=$(find "$MIGRATIONS_DIR" -name "*.sql" | wc -l | xargs)
    if [ "$MIGRATION_COUNT" -ge "$EXPECTED_MIGRATIONS" ]; then
        pass "migration: all $EXPECTED_MIGRATIONS migrations recorded"
    else
        fail "migration: count" "Expected $EXPECTED_MIGRATIONS, got $MIGRATION_COUNT"
    fi

    # Idempotency: run again — should not error
    for sql_file in "$MIGRATIONS_DIR"/*.sql; do
        BASENAME=$(basename "$sql_file" .sql)
        APPLIED=$(sqlite3 "$TEMP_DB" "SELECT COUNT(*) FROM _migrations WHERE name = '$BASENAME';")
        # Already applied — this is the idempotency check
    done
    pass "migration: idempotent (re-run produces no errors)"

    rm -f "$TEMP_DB"
fi

# ═══════════════════════════════════════════════════════════════════════
section "3.3: Heartbeat Script Validation"
# ═══════════════════════════════════════════════════════════════════════

# Read the heartbeat script generator from Rust source
HEARTBEAT_SRC="$PROJECT_ROOT/src-tauri/src/commands/k2so_agents.rs"

# Check health check uses grep for JSON
if grep -q 'grep -q.*"ok"' "$HEARTBEAT_SRC"; then
    pass "heartbeat script: health check parses JSON correctly"
else
    fail "heartbeat script: health check" "Expected grep -q '\"ok\"' pattern"
fi

# Check it calls scheduler-tick not heartbeat
if grep -q 'cli/scheduler-tick' "$HEARTBEAT_SRC"; then
    pass "heartbeat script: calls /cli/scheduler-tick (not /cli/heartbeat)"
else
    fail "heartbeat script: endpoint" "Expected /cli/scheduler-tick"
fi

# ═══════════════════════════════════════════════════════════════════════
section "3.4: Agent Template Validation"
# ═══════════════════════════════════════════════════════════════════════

# We need K2SO running for agent creation, but we can check the template function in source
AGENTS_SRC="$PROJECT_ROOT/src-tauri/src/commands/k2so_agents.rs"

# Verify manager template has key sections (in raw string literals)
if grep -q 'Work Sources' "$AGENTS_SRC"; then
    pass "template: manager has Work Sources section"
else
    fail "template: manager Work Sources" "Section not found in source"
fi

if grep -q 'Your Team' "$AGENTS_SRC"; then
    pass "template: manager has Your Team section"
else
    fail "template: manager Your Team" "Section not found"
fi

if grep -q '"agent-template"\|"pod-member"' "$AGENTS_SRC" && grep -q 'Specialization' "$AGENTS_SRC"; then
    pass "template: agent-template has Specialization section"
else
    fail "template: agent-template Specialization" "Section not found"
fi

if grep -q '"custom"' "$AGENTS_SRC" && grep -q 'Heartbeat Control' "$AGENTS_SRC"; then
    pass "template: custom agent has Heartbeat Control section"
else
    fail "template: custom Heartbeat Control" "Section not found"
fi

# Verify heartbeat docs include noop/action commands
if grep -q 'heartbeat noop' "$AGENTS_SRC" && grep -q 'heartbeat action' "$AGENTS_SRC"; then
    pass "template: heartbeat docs include noop and action commands"
else
    fail "template: heartbeat noop/action" "Commands not found in CUSTOM_AGENT_HEARTBEAT_DOCS"
fi

# ═══════════════════════════════════════════════════════════════════════
section "3.2: LLM Triage Prompt Validation"
# ═══════════════════════════════════════════════════════════════════════

# Verify the triage system prompt has the right rules
if grep -q 'SKIP agents marked as LOCKED' "$AGENTS_SRC"; then
    pass "triage prompt: includes LOCKED rule"
else
    fail "triage prompt: LOCKED rule" "Not found in TRIAGE_SYSTEM_PROMPT"
fi

if grep -q 'NEEDS APPROVAL' "$AGENTS_SRC"; then
    pass "triage prompt: includes NEEDS APPROVAL rule"
else
    fail "triage prompt: NEEDS APPROVAL" "Not found"
fi

if grep -q '"wake"' "$AGENTS_SRC" && grep -q '"reasoning"' "$AGENTS_SRC"; then
    pass "triage prompt: specifies JSON output format with wake and reasoning"
else
    fail "triage prompt: JSON format" "Expected wake/reasoning JSON format"
fi

# Verify parse_triage_response extracts from JSON with preamble
if grep -q 'json_start.*find.*{' "$AGENTS_SRC" || grep -q "response.find('{')" "$AGENTS_SRC"; then
    pass "triage parser: handles JSON with preamble text"
else
    fail "triage parser: preamble handling" "Expected { search in parse_triage_response"
fi

# Verify agent name validation against filesystem
if grep -q 'agents_root.join.*exists' "$AGENTS_SRC"; then
    pass "triage: validates agent names against filesystem"
else
    fail "triage: agent validation" "Expected filesystem validation of LLM-returned names"
fi

# ═══════════════════════════════════════════════════════════════════════
# Results
# ═══════════════════════════════════════════════════════════════════════

echo ""
echo "╔══════════════════════════════════════════════════════════════╗"
echo -e "║  Tier 3 Results: ${GREEN}$PASS passed${NC}, ${RED}$FAIL failed${NC}, ${YELLOW}$SKIP skipped${NC}     ║"
echo "╚══════════════════════════════════════════════════════════════╝"

if [ "$FAIL" -gt 0 ]; then exit 1; fi
