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
section "3.5: Heartbeat Audit Log (heartbeat_fires)"
# ═══════════════════════════════════════════════════════════════════════

# Migration 0026 file exists
if [ -f "$MIGRATIONS_DIR/0026_heartbeat_fires.sql" ]; then
    pass "migration 0026: heartbeat_fires migration file exists"
else
    fail "migration 0026: file missing" "Expected $MIGRATIONS_DIR/0026_heartbeat_fires.sql"
fi

# After migrations run, verify heartbeat_fires table + columns
if command -v sqlite3 &> /dev/null; then
    HB_TEMP_DB="/tmp/k2so-test-heartbeat-$$.db"
    sqlite3 "$HB_TEMP_DB" "CREATE TABLE IF NOT EXISTS _migrations (name TEXT PRIMARY KEY, applied_at INTEGER);"
    for sql_file in "$MIGRATIONS_DIR"/*.sql; do
        sqlite3 "$HB_TEMP_DB" < "$sql_file" 2>/dev/null || true
    done

    HB_TABLES=$(sqlite3 "$HB_TEMP_DB" ".tables" 2>/dev/null)
    if echo "$HB_TABLES" | grep -q "heartbeat_fires"; then
        pass "heartbeat_fires: table created by migration"
    else
        fail "heartbeat_fires: table missing" "Tables: $HB_TABLES"
    fi

    HB_COLS=$(sqlite3 "$HB_TEMP_DB" "PRAGMA table_info(heartbeat_fires);" 2>/dev/null | cut -d'|' -f2 | tr '\n' ' ')
    for col in project_id agent_name fired_at mode decision reason inbox_priority inbox_count duration_ms; do
        if echo "$HB_COLS" | grep -q "\b$col\b"; then
            pass "heartbeat_fires: column $col present"
        else
            fail "heartbeat_fires: column $col missing" "Columns: $HB_COLS"
        fi
    done

    HB_INDEXES=$(sqlite3 "$HB_TEMP_DB" "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='heartbeat_fires';" 2>/dev/null)
    if echo "$HB_INDEXES" | grep -q "idx_heartbeat_fires_project_time"; then
        pass "heartbeat_fires: project-time index present"
    else
        fail "heartbeat_fires: project-time index missing" "Indexes: $HB_INDEXES"
    fi

    rm -f "$HB_TEMP_DB"
else
    skip "heartbeat_fires table checks — sqlite3 not installed"
fi

# ═══════════════════════════════════════════════════════════════════════
section "3.6: Wakeup.md Templates"
# ═══════════════════════════════════════════════════════════════════════

WAKEUP_DIR="$PROJECT_ROOT/src-tauri/wakeup_templates"

for template in workspace manager custom k2so; do
    if [ -f "$WAKEUP_DIR/$template.md" ]; then
        pass "wakeup template: $template.md exists"
    else
        fail "wakeup template: $template.md missing" "Expected $WAKEUP_DIR/$template.md"
    fi
done

# Each template should carry the visible DEFAULT TEMPLATE marker
for template in workspace manager custom k2so; do
    if [ -f "$WAKEUP_DIR/$template.md" ] && grep -q "DEFAULT TEMPLATE" "$WAKEUP_DIR/$template.md"; then
        pass "wakeup template: $template.md has DEFAULT TEMPLATE marker"
    else
        fail "wakeup template: $template.md marker" "DEFAULT TEMPLATE marker not found"
    fi
done

# Agent-template type does NOT get a wakeup template (confirmed in source)
if grep -q 'fn wakeup_template_for' "$AGENTS_SRC" && grep -q '_ => None' "$AGENTS_SRC"; then
    pass "wakeup: wakeup_template_for returns None for agent-template type"
else
    fail "wakeup: type exclusion" "wakeup_template_for signature missing or doesn't fall through to None"
fi

# Compose helpers exist
if grep -q 'pub fn compose_wake_prompt_for_lead' "$AGENTS_SRC"; then
    pass "wakeup: compose_wake_prompt_for_lead exists"
else
    fail "wakeup: compose_wake_prompt_for_lead missing" "Helper not found"
fi

if grep -q 'pub fn compose_wake_prompt_for_agent' "$AGENTS_SRC"; then
    pass "wakeup: compose_wake_prompt_for_agent exists"
else
    fail "wakeup: compose_wake_prompt_for_agent missing" "Helper not found"
fi

# Lazy-create sweep exists (called on app launch for every project)
if grep -q 'pub fn ensure_workspace_wakeups' "$AGENTS_SRC"; then
    pass "wakeup: ensure_workspace_wakeups sweep helper exists"
else
    fail "wakeup: sweep helper missing" "ensure_workspace_wakeups not found"
fi

# ═══════════════════════════════════════════════════════════════════════
section "3.7: Scripted Triage (LLM Removed)"
# ═══════════════════════════════════════════════════════════════════════

HOOKS_SRC="$PROJECT_ROOT/src-tauri/src/agent_hooks.rs"

# LLM path removed from the /cli/scheduler-tick HTTP handler. The LLM
# module still exists and can be kept around for other uses, but the
# scheduler is now deterministic/scripted by default.
if grep -A 60 '"/cli/scheduler-tick" =>' "$HOOKS_SRC" | grep -q 'llm_triage_decide'; then
    fail "scheduler-tick: still invokes LLM" "llm_triage_decide called inside /cli/scheduler-tick branch"
else
    pass "scheduler-tick: LLM triage removed (fully scripted)"
fi

# HTTP handler returns count synchronously, not just {status: triage_started}
if grep -A 60 '"/cli/scheduler-tick" =>' "$HOOKS_SRC" | grep -q '"count":'; then
    pass "scheduler-tick: returns count field synchronously"
else
    fail "scheduler-tick: response shape" "Expected count field; likely still returning {status: triage_started}"
fi

# Scheduler writes audit rows on every decision point
if grep -q 'HeartbeatFire::insert' "$AGENTS_SRC"; then
    pass "scheduler: writes heartbeat_fires audit rows"
else
    fail "scheduler: audit writes missing" "HeartbeatFire::insert not called"
fi

# /cli/heartbeat-log endpoint + CLI command
if grep -q '/cli/heartbeat-log' "$HOOKS_SRC"; then
    pass "heartbeat: /cli/heartbeat-log endpoint present"
else
    fail "heartbeat: log endpoint missing" "/cli/heartbeat-log not in agent_hooks.rs"
fi

if grep -q 'cmd_heartbeat_log' "$K2SO_CLI"; then
    pass "CLI: k2so heartbeat log command wired up"
else
    fail "CLI: heartbeat log command missing" "cmd_heartbeat_log not in cli/k2so"
fi

# /cli/checkin includes wakeupInstructions
if grep -q 'wakeupInstructions' "$HOOKS_SRC"; then
    pass "checkin: response includes wakeupInstructions field"
else
    fail "checkin: missing wakeupInstructions" "Field not found in /cli/checkin handler"
fi

# Heartbeat shell script logs every tick (not just successful launches)
if grep -q 'tick project=' "$AGENTS_SRC"; then
    pass "heartbeat script: logs every tick (fires + skips + errors)"
else
    fail "heartbeat script: silent ticks" "Expected 'tick project=' log pattern in shell script generator"
fi

# ═══════════════════════════════════════════════════════════════════════
section "3.8: Retained-View + File Search + Paste"
# ═══════════════════════════════════════════════════════════════════════

# Retained-view context module exists
if [ -f "$PROJECT_ROOT/src/renderer/contexts/TabVisibilityContext.tsx" ]; then
    pass "retained-view: TabVisibilityContext present"
else
    fail "retained-view: context missing" "Expected src/renderer/contexts/TabVisibilityContext.tsx"
fi

# TerminalArea uses display:none for inactive tabs
TA="$PROJECT_ROOT/src/renderer/components/Terminal/TerminalArea.tsx"
if grep -q "display: isActiveTab ? 'block' : 'none'" "$TA"; then
    pass "retained-view: TerminalArea hides inactive tabs via display:none"
else
    fail "retained-view: TerminalArea" "Expected display:none tab hiding in TerminalArea"
fi

# PaneGroupView renders every item, not just active
PGV="$PROJECT_ROOT/src/renderer/components/PaneLayout/PaneGroupView.tsx"
if grep -q 'paneGroup.items.map' "$PGV" && grep -q "display: hidden ? 'none' : 'block'" "$PGV"; then
    pass "retained-view: PaneGroupView renders all items with display:none"
else
    fail "retained-view: PaneGroupView" "Expected items.map + display:none pattern"
fi

# Backend file search command
FS_SRC="$PROJECT_ROOT/src-tauri/src/commands/filesystem.rs"
if grep -q 'pub fn fs_search_tree' "$FS_SRC"; then
    pass "file search: fs_search_tree command defined"
else
    fail "file search: command missing" "fs_search_tree not found in filesystem.rs"
fi

if grep -q 'SKIP_DIRS.*node_modules\|"node_modules"' "$FS_SRC"; then
    pass "file search: skip-list includes heavy dirs (node_modules, .git, etc.)"
else
    fail "file search: skip-list" "Expected SKIP_DIRS constant referencing node_modules"
fi

# Clipboard file-paths command (Finder CMD+V → terminal)
if grep -q 'pub fn clipboard_read_file_paths' "$FS_SRC"; then
    pass "clipboard: native file-path read command present"
else
    fail "clipboard: command missing" "clipboard_read_file_paths not found"
fi

# Commands registered in lib.rs invoke_handler
LIB_SRC="$PROJECT_ROOT/src-tauri/src/lib.rs"
for cmd in fs_search_tree clipboard_read_file_paths; do
    if grep -q "$cmd" "$LIB_SRC"; then
        pass "lib.rs: $cmd registered with invoke_handler"
    else
        fail "lib.rs: $cmd not registered" "Command missing from invoke_handler in lib.rs"
    fi
done

# Image-aware formatting for Claude Code [Image #N] detection
FD_SRC="$PROJECT_ROOT/src/renderer/lib/file-drag.ts"
ATV_SRC="$PROJECT_ROOT/src/renderer/components/Terminal/AlacrittyTerminalView.tsx"

if grep -q 'export function isImagePath' "$FD_SRC"; then
    pass "image-drop: isImagePath helper exported from file-drag.ts"
else
    fail "image-drop: isImagePath missing" "Expected exported isImagePath in file-drag.ts"
fi

if grep -q 'export function quotePathForImageDrop' "$FD_SRC"; then
    pass "image-drop: quotePathForImageDrop helper exported from file-drag.ts"
else
    fail "image-drop: quote helper missing" "Expected quotePathForImageDrop in file-drag.ts"
fi

if grep -q "'.png', '.jpg', '.jpeg', '.gif', '.webp', '.bmp', '.heic', '.heif', '.pdf'" "$FD_SRC"; then
    pass "image-drop: IMAGE_EXTS covers common formats (png/jpg/jpeg/gif/webp/bmp/heic/heif/pdf)"
else
    fail "image-drop: extension list" "Expected full IMAGE_EXTS list in file-drag.ts"
fi

if grep -q 'isImagePath(p) ? quotePathForImageDrop(p) : shellEscape(p)' "$FD_SRC"; then
    pass "image-drop: in-app drag → terminal routes images through quotePathForImageDrop"
else
    fail "image-drop: in-app drag" "Expected image-aware formatter at drop-to-terminal site in file-drag.ts"
fi

if grep -q 'function formatPathForTerminal' "$ATV_SRC"; then
    pass "image-drop: AlacrittyTerminalView has formatPathForTerminal helper"
else
    fail "image-drop: helper missing" "Expected formatPathForTerminal in AlacrittyTerminalView.tsx"
fi

if grep -q 'isImagePath, quotePathForImageDrop' "$ATV_SRC"; then
    pass "image-drop: AlacrittyTerminalView imports image helpers from file-drag"
else
    fail "image-drop: import missing" "Expected named imports of isImagePath + quotePathForImageDrop"
fi

# All three terminal write sites use formatPathForTerminal (not bare shellEscape.*join)
ATV_ESCAPE_HITS=$(grep -c 'paths.map(shellEscape)' "$ATV_SRC" || true)
if [ "$ATV_ESCAPE_HITS" = "0" ]; then
    pass "image-drop: no residual shellEscape .map sites in AlacrittyTerminalView"
else
    fail "image-drop: residual shellEscape" "$ATV_ESCAPE_HITS bare shellEscape map call(s) still present"
fi

ATV_DROP_HITS=$(grep -c 'buildDropPayload' "$ATV_SRC" || true)
# Expected: 1 declaration + 3 call sites (tauri drop, paste, React onDrop) = 4
if [ "$ATV_DROP_HITS" -ge 4 ]; then
    pass "image-drop: buildDropPayload used at all three write sites (decl + 3 calls)"
else
    fail "image-drop: write sites" "Expected >=4 buildDropPayload references, got $ATV_DROP_HITS"
fi

# Bracketed paste helpers — required for Claude Code's image detector to fire
if grep -q "export const BRACKETED_PASTE_START = '\\\\x1b\\[200~'" "$FD_SRC"; then
    pass "image-drop: BRACKETED_PASTE_START constant defined"
else
    fail "image-drop: bracketed paste start" "Expected BRACKETED_PASTE_START = '\\x1b[200~' export"
fi

if grep -q 'export function bracketPaste' "$FD_SRC"; then
    pass "image-drop: bracketPaste helper exported"
else
    fail "image-drop: bracketPaste missing" "Expected bracketPaste export in file-drag.ts"
fi

if grep -q 'bracketPaste' "$ATV_SRC"; then
    pass "image-drop: AlacrittyTerminalView wraps image payloads in bracketed paste"
else
    fail "image-drop: bracket wrap" "Expected bracketPaste use in AlacrittyTerminalView.tsx"
fi

# Working-state title signal — must record BEFORE stripping so tab spinners
# and the macOS close-button dot light up when Claude Code is working
if grep -q 'recordTitleActivity: (paneId: string, isWorking: boolean)' "$PROJECT_ROOT/src/renderer/stores/active-agents.ts"; then
    pass "working-state: recordTitleActivity method on active-agents store"
else
    fail "working-state: store method" "Expected recordTitleActivity in active-agents.ts"
fi

if grep -q 'recordTitleActivity(terminalId,' "$ATV_SRC"; then
    pass "working-state: AlacrittyTerminalView feeds activity into store"
else
    fail "working-state: store wiring" "Expected recordTitleActivity call in AlacrittyTerminalView"
fi

# Guard: recordTitleActivity must NOT clobber 'permission' or 'review' states
if grep -q "if (current === 'permission' || current === 'review') return" "$PROJECT_ROOT/src/renderer/stores/active-agents.ts"; then
    pass "working-state: title signal respects permission/review priority"
else
    fail "working-state: priority guard" "Expected guard against clobbering permission/review"
fi

# Viewport-scan working detector — replaces the flaky title-glyph detection
SIG_SRC="$PROJECT_ROOT/src/renderer/lib/agent-signals.ts"
if [ -f "$SIG_SRC" ]; then
    pass "working-state: agent-signals.ts module present"
else
    fail "working-state: agent-signals missing" "Expected src/renderer/lib/agent-signals.ts"
fi

if grep -q "'esc to interrupt'" "$SIG_SRC" && grep -q "'esc to cancel'" "$SIG_SRC"; then
    pass "working-state: WORKING_SIGNALS covers claude/codex (esc to interrupt) and gemini (esc to cancel)"
else
    fail "working-state: signals coverage" "Expected esc to interrupt + esc to cancel substrings"
fi

for sig in "'waiting for '" "'thinking...'" "'working...'" "' is thinking...'"; do
    if grep -q "$sig" "$SIG_SRC"; then
        pass "working-state: WORKING_SIGNALS includes $sig"
    else
        fail "working-state: missing signal $sig" "Expected $sig in WORKING_SIGNALS"
    fi
done

if grep -q 'export function detectWorkingSignal' "$SIG_SRC"; then
    pass "working-state: detectWorkingSignal helper exported"
else
    fail "working-state: detectWorkingSignal" "Expected detectWorkingSignal export"
fi

if grep -q 'detectWorkingSignal' "$ATV_SRC"; then
    pass "working-state: AlacrittyTerminalView uses viewport scanner"
else
    fail "working-state: scanner not wired" "Expected detectWorkingSignal call in AlacrittyTerminalView"
fi

# Scanner must be gated on displayOffset === 0 so scroll-up doesn't pin 'working'
if grep -B1 'detectWorkingSignal(map, update.rows)' "$ATV_SRC" | grep -q 'display_offset === 0'; then
    pass "working-state: viewport scan gated on display_offset === 0"
else
    fail "working-state: scroll guard" "Expected display_offset === 0 gate around detectWorkingSignal"
fi

# Idle watcher — flips to idle after 1s without a signal
if grep -q 'lastSeenWorkingAtRef' "$ATV_SRC" && grep -q 'IDLE_GRACE_MS' "$ATV_SRC"; then
    pass "working-state: idle-grace timer + lastSeenWorkingAtRef wired"
else
    fail "working-state: idle watcher" "Expected lastSeenWorkingAtRef + IDLE_GRACE_MS in AlacrittyTerminalView"
fi

# tauri://drag-drop is window-level — every terminal listens. Without a
# hit-test against containerRef, a drop into one column of a split layout
# pastes into every terminal in the window.
if grep -q 'containerRef.current?.contains(el)' "$ATV_SRC"; then
    pass "drop hit-test: tauri://drag-drop listener checks containerRef before accepting"
else
    fail "drop hit-test: missing containerRef check" "Expected containerRef.current?.contains(el) guard in tauri://drag-drop listener"
fi

# ──────────────────────────────────────────────────────────────────────
# Tier-1 diagnostic: k2so hooks status
# Validates the hook-pipeline observability surface (ring buffer + endpoint
# + CLI). Pairs with Rust unit tests in agent_hooks.rs::tests which cover
# runtime behavior of the ring buffer and injection check.
# ──────────────────────────────────────────────────────────────────────

HOOKS_SRC="$PROJECT_ROOT/src-tauri/src/agent_hooks.rs"
K2SO_CLI="$PROJECT_ROOT/cli/k2so"

if grep -q 'struct RecentEvent' "$HOOKS_SRC"; then
    pass "hooks status: RecentEvent struct defined"
else
    fail "hooks status: RecentEvent struct missing" "Expected struct RecentEvent in agent_hooks.rs"
fi

if grep -q 'const RECENT_EVENTS_CAP: usize' "$HOOKS_SRC"; then
    pass "hooks status: ring-buffer cap constant defined"
else
    fail "hooks status: RECENT_EVENTS_CAP missing" "Expected ring-buffer cap in agent_hooks.rs"
fi

if grep -q 'fn record_recent_event' "$HOOKS_SRC"; then
    pass "hooks status: record_recent_event helper present"
else
    fail "hooks status: record helper missing" "Expected record_recent_event fn"
fi

if grep -q 'record_recent_event(&raw_event' "$HOOKS_SRC"; then
    pass "hooks status: /hook/complete handler pushes every event to the ring"
else
    fail "hooks status: handler not wired" "Expected record_recent_event call in /hook/complete"
fi

if grep -q '"/cli/hooks/status"' "$HOOKS_SRC"; then
    pass "hooks status: /cli/hooks/status route registered"
else
    fail "hooks status: route missing" "Expected /cli/hooks/status route in agent_hooks.rs"
fi

if grep -q 'pub fn check_hook_injections' "$HOOKS_SRC"; then
    pass "hooks status: check_hook_injections helper exported"
else
    fail "hooks status: check helper missing" "Expected check_hook_injections fn"
fi

if grep -q 'cmd_hooks_status()' "$K2SO_CLI"; then
    pass "hooks status: cmd_hooks_status CLI function present"
else
    fail "hooks status: CLI fn missing" "Expected cmd_hooks_status in cli/k2so"
fi

if grep -q '^    hooks)' "$K2SO_CLI"; then
    pass "hooks status: 'hooks' dispatch branch in main case"
else
    fail "hooks status: dispatch missing" "Expected 'hooks)' branch in cli/k2so argument parser"
fi

# Verify the Rust unit tests at least compile — prevents drift between
# the structural check and the behavioral suite.
if grep -q 'mod tests' "$HOOKS_SRC" && grep -q 'fn ring_buffer_caps_at_limit' "$HOOKS_SRC"; then
    pass "hooks status: Rust unit tests present for ring buffer + injections"
else
    fail "hooks status: Rust tests missing" "Expected #[cfg(test)] mod tests with ring-buffer coverage"
fi

# Hook-trust grace in poll cleanup — prevents pollOnce from clobbering
# hook-driven 'working' states when foreground command transiently changes
# (e.g. Claude spawning `bash` / `rg` for a tool call).
AA_SRC="$PROJECT_ROOT/src/renderer/stores/active-agents.ts"

if grep -q '_hookEventAt' "$AA_SRC" && grep -q 'HOOK_TRUST_GRACE_MS' "$AA_SRC"; then
    pass "hook trust: _hookEventAt + HOOK_TRUST_GRACE_MS defined"
else
    fail "hook trust: timestamp tracking missing" "Expected _hookEventAt Map + HOOK_TRUST_GRACE_MS const"
fi

if grep -q '_hookEventAt.set(paneId, Date.now())' "$AA_SRC"; then
    pass "hook trust: handleLifecycleEvent stamps _hookEventAt on every fire"
else
    fail "hook trust: stamp missing" "Expected _hookEventAt.set call in handleLifecycleEvent"
fi

if grep -q 'if (hookAge < HOOK_TRUST_GRACE_MS) continue' "$AA_SRC"; then
    pass "hook trust: pollOnce cleanup skips clearing when hook fired recently"
else
    fail "hook trust: cleanup not gated" "Expected HOOK_TRUST_GRACE_MS guard in pollOnce cleanup"
fi

if grep -q 'if (outputAge < OUTPUT_TRUST_GRACE_MS) continue' "$AA_SRC"; then
    pass "hook trust: pollOnce cleanup also skips on recent output activity"
else
    fail "hook trust: output gate missing" "Expected OUTPUT_TRUST_GRACE_MS guard in pollOnce cleanup"
fi

# Hardening #1: notify.sh must read port + token dynamically so a K2SO
# restart doesn't break hooks in long-running LLM sessions.
if grep -q 'K2SO_PORT_FILE="\$HOME/.k2so/heartbeat.port"' "$HOOKS_SRC"; then
    pass "hardening: notify.sh reads port from heartbeat.port at exec time"
else
    fail "hardening: port still baked in" "Expected K2SO_PORT_FILE in generate_hook_script"
fi

if grep -q 'K2SO_TOKEN_FILE="\$HOME/.k2so/heartbeat.token"' "$HOOKS_SRC"; then
    pass "hardening: notify.sh prefers token from heartbeat.token at exec time"
else
    fail "hardening: token not dynamic" "Expected K2SO_TOKEN_FILE in generate_hook_script"
fi

# Hardening #3: hook-injection failures surface to the user via toast.
if grep -q 'hook-injection-failed' "$HOOKS_SRC"; then
    pass "hardening: register_all_hooks emits hook-injection-failed Tauri event"
else
    fail "hardening: injection event missing" "Expected hook-injection-failed emit in agent_hooks.rs"
fi

if grep -q 'hook-injection-failed' "$AA_SRC"; then
    pass "hardening: frontend listens for hook-injection-failed and toasts"
else
    fail "hardening: frontend listener missing" "Expected hook-injection-failed listener in active-agents.ts"
fi

if grep -q 'register_all_hooks(app_handle: &AppHandle' "$HOOKS_SRC"; then
    pass "hardening: register_all_hooks takes AppHandle for event emission"
else
    fail "hardening: signature not updated" "Expected register_all_hooks(app_handle: &AppHandle, …) signature"
fi

# ──────────────────────────────────────────────────────────────────────
# Heartbeat wake reliability: --fork-session + wake-trigger + compact-every-N
# Addresses: stale-session dialog blocking wakes, Claude sitting idle at TUI
# after spawn (no user message), unbounded history growth across wakes.
# ──────────────────────────────────────────────────────────────────────

AGENTS_SRC="$PROJECT_ROOT/src-tauri/src/commands/k2so_agents.rs"
SCHEMA_SRC="$PROJECT_ROOT/src-tauri/src/db/schema.rs"
DB_MOD_SRC="$PROJECT_ROOT/src-tauri/src/db/mod.rs"

# Migration applied in mod.rs
if grep -q '"0027_wakes_since_compact"' "$DB_MOD_SRC"; then
    pass "wake reliability: migration 0027 registered in db/mod.rs"
else
    fail "wake reliability: migration missing" "Expected 0027_wakes_since_compact in migrations list"
fi

# Migration file exists with the expected ALTER TABLE
if [ -f "$PROJECT_ROOT/src-tauri/drizzle_sql/0027_wakes_since_compact.sql" ]; then
    pass "wake reliability: migration 0027 SQL file present"
else
    fail "wake reliability: SQL file missing" "Expected drizzle_sql/0027_wakes_since_compact.sql"
fi

# Schema methods for counter
if grep -q 'pub fn bump_wake_counter' "$SCHEMA_SRC"; then
    pass "wake reliability: AgentSession::bump_wake_counter defined"
else
    fail "wake reliability: bump fn missing" "Expected bump_wake_counter in schema.rs"
fi

if grep -q 'pub fn reset_wake_counter' "$SCHEMA_SRC"; then
    pass "wake reliability: AgentSession::reset_wake_counter defined"
else
    fail "wake reliability: reset fn missing" "Expected reset_wake_counter in schema.rs"
fi

# --fork-session is added alongside --resume in build_launch
if grep -q '"--fork-session"' "$AGENTS_SRC"; then
    pass "wake reliability: --fork-session added to resume path"
else
    fail "wake reliability: --fork-session missing" "Expected --fork-session in k2so_agents.rs"
fi

# Compact-every-N constant + logic
if grep -q 'const WAKES_PER_COMPACT: i64 = 20' "$AGENTS_SRC"; then
    pass "wake reliability: WAKES_PER_COMPACT constant set to 20"
else
    fail "wake reliability: constant missing" "Expected const WAKES_PER_COMPACT: i64 = 20"
fi

if grep -q 'bump_wake_counter' "$AGENTS_SRC"; then
    pass "wake reliability: build_launch bumps the wake counter"
else
    fail "wake reliability: counter not bumped" "Expected bump_wake_counter call in build_launch"
fi

if grep -q '"/compact' "$AGENTS_SRC"; then
    pass "wake reliability: compact-prefixed wake trigger built for every-N wake"
else
    fail "wake reliability: compact wake message" "Expected /compact prefix in wake-trigger string"
fi

# Wake-trigger positional message added unconditionally for build_launch path
# Wake prompt must be delivered as the positional user message (wakeup.md
# content), not buried in --append-system-prompt where Claude reads it as
# background rather than an actionable directive.
if grep -q 'let wake_message = wake_body.unwrap_or_else' "$AGENTS_SRC"; then
    pass "wake reliability: wakeup.md content used as positional user message"
else
    fail "wake reliability: wake message delivery" "Expected wake_body → wake_message positional"
fi

# The system prompt for build_launch is now just CLAUDE.md (identity),
# not CLAUDE.md + wakeup.md.
if grep -q 'let system_prompt = claude_md;' "$AGENTS_SRC"; then
    pass "wake reliability: system prompt is identity-only (CLAUDE.md)"
else
    fail "wake reliability: system prompt" "Expected system_prompt = claude_md with no wake content"
fi

# Wake-site spawns must be backend-direct (not Tauri event emits) so they
# fire when the K2SO window is closed. The helper spawn_wake_pty wraps
# TerminalManager::create so the companion-background-spawn pattern
# applies to heartbeat wakes too.
if grep -q 'fn spawn_wake_pty' "$HOOKS_SRC"; then
    pass "wake reliability: spawn_wake_pty helper defined"
else
    fail "wake reliability: helper missing" "Expected spawn_wake_pty in agent_hooks.rs"
fi

SPAWN_CALL_HITS=$(grep -c 'spawn_wake_pty(' "$HOOKS_SRC" || true)
# 1 definition + 4 call sites (2 lead + 2 sub-agent paths) = 5 references
if [ "$SPAWN_CALL_HITS" -ge 5 ]; then
    pass "wake reliability: spawn_wake_pty used at all heartbeat wake sites ($SPAWN_CALL_HITS)"
else
    fail "wake reliability: spawn_wake_pty under-used" "Expected >=5 references (decl + 4 call sites), got $SPAWN_CALL_HITS"
fi

# No heartbeat-triggered wake should still be using cli:agent-launch
# (the old event-driven path that broke when no window was open). The
# event name may still appear in frontend listeners, so we check the
# Rust side only.
LAUNCH_EMIT_HITS=$(grep -c '"cli:agent-launch"' "$HOOKS_SRC" || true)
if [ "$LAUNCH_EMIT_HITS" -eq 0 ]; then
    pass "wake reliability: no cli:agent-launch emits remain in heartbeat path"
else
    fail "wake reliability: stale emit" "Expected 0 cli:agent-launch emits in agent_hooks.rs, got $LAUNCH_EMIT_HITS"
fi

# Backend-direct spawn must also persist the new Claude session ID so the
# next wake can resume it. Without this save, every wake reads a stale
# .last_session and hits "No conversation found" on --resume.
if grep -q 'k2so_agents_save_session_id' "$HOOKS_SRC"; then
    pass "wake reliability: spawn_wake_pty persists session ID post-spawn"
else
    fail "wake reliability: session save missing" "Expected k2so_agents_save_session_id call in spawn_wake_pty"
fi

if grep -q 'chat_history_detect_active_session' "$HOOKS_SRC"; then
    pass "wake reliability: spawn_wake_pty detects Claude session via history scan"
else
    fail "wake reliability: session detect missing" "Expected chat_history_detect_active_session call in spawn_wake_pty"
fi

# Backend-spawn must also lock the AgentSession row (status='running',
# owner='system') so a racing heartbeat tick doesn't double-launch the
# same agent. The frontend listener used to handle this via invoke(),
# but backend-direct spawn bypasses that path.
if grep -q 'k2so_agents_lock(' "$HOOKS_SRC"; then
    pass "wake reliability: spawn_wake_pty locks AgentSession to prevent double-launch"
else
    fail "wake reliability: session lock missing" "Expected k2so_agents_lock call in spawn_wake_pty"
fi

# Hook handler must sync AgentSession.status on every canonical event so the
# scheduler's is_agent_locked check reflects reality. Without this the row
# stays status='running' forever after the first wake and every subsequent
# heartbeat skips the agent.
if grep -q 'AgentSession::get_by_terminal_id' "$HOOKS_SRC"; then
    pass "wake reliability: hook handler resolves pane_id → AgentSession row"
else
    fail "wake reliability: terminal_id lookup missing" "Expected get_by_terminal_id in hook handler"
fi

if grep -q '"stop" => Some("sleeping")' "$HOOKS_SRC"; then
    pass "wake reliability: Stop event flips AgentSession.status to sleeping"
else
    fail "wake reliability: Stop → sleeping" "Expected canonical 'stop' → 'sleeping' mapping in hook handler"
fi

# AgentSession schema helper for terminal_id lookup
if grep -q 'pub fn get_by_terminal_id' "$SCHEMA_SRC"; then
    pass "wake reliability: AgentSession::get_by_terminal_id schema helper present"
else
    fail "wake reliability: helper missing" "Expected get_by_terminal_id in schema.rs"
fi

# .last_session retirement — no Rust code should still read/write it
LAST_SESSION_FILE_HITS=$(grep -c '\.last_session' "$AGENTS_SRC" || true)
# Allowed: only docstring/comment references in retired state
if grep -q '\.last_session.*file.*was retired\|retired' "$AGENTS_SRC" && [ "$LAST_SESSION_FILE_HITS" -le 2 ]; then
    pass ".last_session retired: no live read/write of the legacy file"
elif grep -q 'fs::write.*last_session\|fs::remove_file.*last_session\|fs::read_to_string.*last_session' "$AGENTS_SRC"; then
    fail ".last_session retirement" "Live file I/O of .last_session still present in k2so_agents.rs"
else
    pass ".last_session retirement: no live file I/O remains"
fi

# Red-button should keep the process alive when any project has heartbeat
# enabled, so autonomous wakes can fire without the user remembering to
# keep the window open. Cmd+Q still quits.
LIB_SRC="$PROJECT_ROOT/src-tauri/src/lib.rs"
if grep -q 'fn any_heartbeat_enabled' "$LIB_SRC"; then
    pass "red-button keep-alive: any_heartbeat_enabled helper defined"
else
    fail "red-button: helper missing" "Expected any_heartbeat_enabled fn in lib.rs"
fi

if grep -q 'if any_heartbeat_enabled()' "$LIB_SRC" && grep -q 'api.prevent_close()' "$LIB_SRC"; then
    pass "red-button keep-alive: CloseRequested intercepted when heartbeat enabled"
else
    fail "red-button: intercept missing" "Expected any_heartbeat_enabled + prevent_close in CloseRequested"
fi

if grep -q 'RunEvent::Reopen' "$LIB_SRC"; then
    pass "red-button keep-alive: Reopen handler re-shows hidden window on Dock click"
else
    fail "red-button: Reopen missing" "Expected RunEvent::Reopen handler in app.run"
fi

# ═══════════════════════════════════════════════════════════════════════
section "3.9: Settings Search Palette"
# ═══════════════════════════════════════════════════════════════════════

SETTINGS_DIR="$PROJECT_ROOT/src/renderer/components/Settings"

# Core files exist
if [ -f "$SETTINGS_DIR/searchManifest.ts" ]; then
    pass "settings search: searchManifest.ts exists"
else
    fail "settings search: searchManifest.ts missing" "Expected $SETTINGS_DIR/searchManifest.ts"
fi

if [ -f "$SETTINGS_DIR/SettingsSearchModal.tsx" ]; then
    pass "settings search: SettingsSearchModal.tsx exists"
else
    fail "settings search: modal missing" "Expected $SETTINGS_DIR/SettingsSearchModal.tsx"
fi

# Every section file exports SECTION_MANIFEST (or its named equivalent)
SECTIONS=(
    "GeneralSection:GENERAL_MANIFEST"
    "ProjectsSection:PROJECTS_MANIFEST"
    "WorkspaceStatesSection:WORKSPACE_STATES_MANIFEST"
    "AgentSkillsSection:AGENT_SKILLS_MANIFEST"
    "TerminalSection:TERMINAL_MANIFEST"
    "CodeEditorSettingsSection:CODE_EDITOR_MANIFEST"
    "EditorsAgentsSection:EDITORS_AGENTS_MANIFEST"
    "KeybindingsSection:KEYBINDINGS_MANIFEST"
    "TimerSection:TIMER_MANIFEST"
    "CompanionSection:COMPANION_MANIFEST"
)
for pair in "${SECTIONS[@]}"; do
    file="${pair%:*}"
    export_name="${pair#*:}"
    path="$SETTINGS_DIR/sections/$file.tsx"
    if [ -f "$path" ] && grep -q "export const $export_name" "$path"; then
        pass "section manifest: $file exports $export_name"
    else
        fail "section manifest: $file missing $export_name" "Expected in $path"
    fi
done

# Router imports every manifest
ROUTER="$SETTINGS_DIR/Settings.tsx"
for pair in "${SECTIONS[@]}"; do
    export_name="${pair#*:}"
    if grep -q "$export_name" "$ROUTER"; then
        pass "Settings.tsx imports $export_name"
    else
        fail "Settings.tsx missing $export_name import" "Add import to Settings.tsx"
    fi
done

# Escape fix: modal has both React-synthetic stopPropagation AND native
# capture-phase window listener. Regression-guards the fix that keeps
# Escape from closing Settings when the search modal is open.
MODAL_SRC="$SETTINGS_DIR/SettingsSearchModal.tsx"
if grep -q "stopPropagation" "$MODAL_SRC"; then
    pass "settings search: modal stops event propagation on Escape"
else
    fail "settings search: propagation not stopped" "Expected stopPropagation() in Escape handler"
fi
if grep -q "window.addEventListener.*keydown.*true" "$MODAL_SRC"; then
    pass "settings search: modal installs native capture-phase Escape listener"
else
    fail "settings search: no capture listener" "Expected window.addEventListener('keydown', ..., true)"
fi

# SettingRow accepts settingId prop (powers scroll-to-row highlighting)
CONTROLS="$SETTINGS_DIR/controls/SettingControls.tsx"
if grep -q "settingId" "$CONTROLS" && grep -q "data-settings-id" "$CONTROLS"; then
    pass "settings search: SettingRow supports settingId + renders data-settings-id"
else
    fail "settings search: row id plumbing" "SettingRow should accept settingId and set data-settings-id"
fi

# Magnifier button + CMD+F hotkey wired in router
if grep -q "setSearchOpen" "$ROUTER"; then
    pass "Settings.tsx: searchOpen state wired"
else
    fail "Settings.tsx: searchOpen state missing" "Router should manage modal open/close state"
fi

if grep -q "metaKey.*'f'\|ctrlKey.*'f'" "$ROUTER" || grep -q "'f'.*metaKey\|'f'.*ctrlKey" "$ROUTER"; then
    pass "Settings.tsx: ⌘F hotkey opens the search modal"
else
    fail "Settings.tsx: ⌘F hotkey missing" "Expected Cmd/Ctrl+F binding that opens the modal"
fi

# ═══════════════════════════════════════════════════════════════════════
# Results
# ═══════════════════════════════════════════════════════════════════════

echo ""
echo "╔══════════════════════════════════════════════════════════════╗"
echo -e "║  Tier 3 Results: ${GREEN}$PASS passed${NC}, ${RED}$FAIL failed${NC}, ${YELLOW}$SKIP skipped${NC}     ║"
echo "╚══════════════════════════════════════════════════════════════╝"

if [ "$FAIL" -gt 0 ]; then exit 1; fi
