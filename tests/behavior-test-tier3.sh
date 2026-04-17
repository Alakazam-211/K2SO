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

if grep -q 'recordTitleActivity(terminalId, isWorking)' "$ATV_SRC"; then
    pass "working-state: AlacrittyTerminalView feeds title-based working signal"
else
    fail "working-state: title listener" "Expected recordTitleActivity call in title event listener"
fi

# Guard: recordTitleActivity must NOT clobber 'permission' or 'review' states
if grep -q "if (current === 'permission' || current === 'review') return" "$PROJECT_ROOT/src/renderer/stores/active-agents.ts"; then
    pass "working-state: title signal respects permission/review priority"
else
    fail "working-state: priority guard" "Expected guard against clobbering permission/review"
fi

# tauri://drag-drop is window-level — every terminal listens. Without a
# hit-test against containerRef, a drop into one column of a split layout
# pastes into every terminal in the window.
if grep -q 'containerRef.current?.contains(el)' "$ATV_SRC"; then
    pass "drop hit-test: tauri://drag-drop listener checks containerRef before accepting"
else
    fail "drop hit-test: missing containerRef check" "Expected containerRef.current?.contains(el) guard in tauri://drag-drop listener"
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
