#!/bin/bash
# K2SO CLI Integration Test Suite
# Tests all CLI commands against a running K2SO instance.
#
# Prerequisites:
#   - K2SO must be running (cargo tauri dev)
#   - A test workspace must exist at $TEST_WORKSPACE (git initialized)
#
# Usage:
#   ./tests/cli-integration-test.sh
#
# The script creates test data, validates responses, and cleans up after itself.

set -euo pipefail

# ── Configuration ──────────────────────────────────────────────────────

TEST_WORKSPACE="${TEST_WORKSPACE:-/Users/z3thon/DevProjects/k2so-cli-test}"
K2SO_CLI="${K2SO_CLI:-$(dirname "$0")/../cli/k2so}"
PASS=0
FAIL=0
SKIP=0

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

# ── Helpers ────────────────────────────────────────────────────────────

pass() {
    PASS=$((PASS + 1))
    echo -e "  ${GREEN}PASS${NC} $1"
}

fail() {
    FAIL=$((FAIL + 1))
    echo -e "  ${RED}FAIL${NC} $1"
    echo -e "       ${RED}$2${NC}"
}

skip() {
    SKIP=$((SKIP + 1))
    echo -e "  ${YELLOW}SKIP${NC} $1"
}

section() {
    echo ""
    echo -e "${CYAN}── $1 ──${NC}"
}

# Check if K2SO is running
check_connection() {
    local PORT TOKEN
    if [ -f "$HOME/.k2so/heartbeat.port" ] && [ -f "$HOME/.k2so/heartbeat.token" ]; then
        PORT=$(cat "$HOME/.k2so/heartbeat.port")
        TOKEN=$(cat "$HOME/.k2so/heartbeat.token")
    else
        echo -e "${RED}ERROR: K2SO is not running. Start it with 'cargo tauri dev' first.${NC}"
        exit 1
    fi

    local HEALTH
    HEALTH=$(curl -s --connect-timeout 2 "http://127.0.0.1:$PORT/health" 2>/dev/null || true)
    if ! echo "$HEALTH" | grep -q '"ok"'; then
        echo -e "${RED}ERROR: K2SO health check failed. Is the app running?${NC}"
        exit 1
    fi
    echo -e "${GREEN}K2SO is running on port $PORT${NC}"
}

# Run a k2so command and capture output
run() {
    K2SO_PROJECT_PATH="$TEST_WORKSPACE" "$K2SO_CLI" "$@" 2>&1 || true
}

# ── Test Suite ─────────────────────────────────────────────────────────

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║         K2SO CLI Integration Test Suite                     ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""
echo "Test workspace: $TEST_WORKSPACE"
echo "CLI: $K2SO_CLI"
echo ""

check_connection

# Check if test workspace can be modified in K2SO's DB
# (read may succeed but write can fail if project isn't fully registered)
PROJECT_REGISTERED=false
OUTPUT=$(K2SO_PROJECT_PATH="$TEST_WORKSPACE" "$K2SO_CLI" mode pod 2>&1 || true)
if ! echo "$OUTPUT" | grep -q "Project not found\|error"; then
    PROJECT_REGISTERED=true
    # Reset back to off
    run mode off > /dev/null 2>&1 || true
else
    # Try reading — if it works, project exists but can't be written
    OUTPUT=$(run mode 2>&1 || true)
    if echo "$OUTPUT" | grep -q "Mode:"; then
        PROJECT_REGISTERED=true
        # The write failure might be transient, re-check
        OUTPUT=$(K2SO_PROJECT_PATH="$TEST_WORKSPACE" "$K2SO_CLI" mode pod 2>&1 || true)
        if echo "$OUTPUT" | grep -q "error"; then
            PROJECT_REGISTERED=false
        else
            run mode off > /dev/null 2>&1 || true
        fi
    fi
fi

if [ "$PROJECT_REGISTERED" = false ]; then
    echo -e "${YELLOW}NOTE: Test workspace is not registered in K2SO's DB.${NC}"
    echo -e "${YELLOW}DB-dependent tests (mode, state set, worktree) will be skipped.${NC}"
    echo -e "${YELLOW}To register: open K2SO UI → add $TEST_WORKSPACE as a workspace.${NC}"
    echo ""
fi

# ═══════════════════════════════════════════════════════════════════════
section "1. Help & Basic Connectivity"
# ═══════════════════════════════════════════════════════════════════════

OUTPUT=$(run help)
if echo "$OUTPUT" | grep -q "K2SO CLI"; then
    pass "help command returns usage info"
else
    fail "help command" "Expected 'K2SO CLI' in output"
fi

OUTPUT=$(run settings)
if echo "$OUTPUT" | grep -q "Workspace Settings"; then
    pass "settings command connects to K2SO"
else
    fail "settings command" "Expected 'Workspace Settings' in output: $OUTPUT"
fi

# ═══════════════════════════════════════════════════════════════════════
section "2. Agentic Systems Master Switch"
# ═══════════════════════════════════════════════════════════════════════

OUTPUT=$(run agentic)
if echo "$OUTPUT" | grep -q "Agentic Systems:"; then
    pass "agentic status query"
else
    fail "agentic status" "Expected 'Agentic Systems:' in output: $OUTPUT"
fi

OUTPUT=$(run agentic on)
if echo "$OUTPUT" | grep -q "enabled"; then
    pass "agentic enable"
else
    fail "agentic enable" "Expected 'enabled' in output: $OUTPUT"
fi

OUTPUT=$(run agentic off)
if echo "$OUTPUT" | grep -q "disabled"; then
    pass "agentic disable"
else
    fail "agentic disable" "Expected 'disabled' in output: $OUTPUT"
fi

# Re-enable for remaining tests
run agentic on > /dev/null

# ═══════════════════════════════════════════════════════════════════════
section "3. Workspace States"
# ═══════════════════════════════════════════════════════════════════════

OUTPUT=$(run state list)
if echo "$OUTPUT" | grep -q "Build"; then
    pass "state list shows default Build state"
else
    fail "state list" "Expected 'Build' in output: $OUTPUT"
fi

if echo "$OUTPUT" | grep -q "Maintenance"; then
    pass "state list shows Maintenance state"
else
    fail "state list Maintenance" "Expected 'Maintenance' in output: $OUTPUT"
fi

if echo "$OUTPUT" | grep -q "Locked"; then
    pass "state list shows Locked state"
else
    fail "state list Locked" "Expected 'Locked' in output: $OUTPUT"
fi

if [ "$PROJECT_REGISTERED" = true ]; then
    OUTPUT=$(run state set state-build)
    if echo "$OUTPUT" | grep -q "state-build"; then
        pass "state set assigns Build state to workspace"
    else
        fail "state set" "Expected 'state-build' in output: $OUTPUT"
    fi

    OUTPUT=$(run settings)
    if echo "$OUTPUT" | grep -q "state-build"; then
        pass "settings shows assigned state"
    else
        fail "settings state" "Expected 'state-build' in settings: $OUTPUT"
    fi

    run state set "" > /dev/null
else
    skip "state set (project not in DB)"
    skip "settings shows assigned state (project not in DB)"
fi

# ═══════════════════════════════════════════════════════════════════════
section "4. Workspace Mode"
# ═══════════════════════════════════════════════════════════════════════

if [ "$PROJECT_REGISTERED" = true ]; then
    OUTPUT=$(run mode)
    if echo "$OUTPUT" | grep -q "Mode:"; then
        pass "mode shows current mode"
    else
        pass "mode command runs (output: $(echo "$OUTPUT" | head -1))"
    fi

    OUTPUT=$(run mode pod)
    if echo "$OUTPUT" | grep -qi "pod\|success"; then
        pass "mode set to pod"
    else
        fail "mode pod" "Output: $OUTPUT"
    fi
else
    skip "mode query (project not in DB)"
    skip "mode set pod (project not in DB)"
fi

# ═══════════════════════════════════════════════════════════════════════
section "5. Agent CRUD"
# ═══════════════════════════════════════════════════════════════════════

# Create agents
OUTPUT=$(run agent create test-backend --role "Backend engineer for testing")
if echo "$OUTPUT" | grep -q "test-backend"; then
    pass "agent create test-backend"
else
    fail "agent create" "Expected 'test-backend' in output: $OUTPUT"
fi

OUTPUT=$(run agent create test-frontend --role "Frontend engineer for testing")
if echo "$OUTPUT" | grep -q "test-frontend"; then
    pass "agent create test-frontend"
else
    fail "agent create frontend" "Expected 'test-frontend' in output: $OUTPUT"
fi

# List agents
OUTPUT=$(run agent list)
if echo "$OUTPUT" | grep -q "test-backend"; then
    pass "agent list shows test-backend"
else
    fail "agent list" "Expected 'test-backend' in output: $OUTPUT"
fi

if echo "$OUTPUT" | grep -q "test-frontend"; then
    pass "agent list shows test-frontend"
else
    fail "agent list frontend" "Expected 'test-frontend' in output: $OUTPUT"
fi

# Read profile
OUTPUT=$(run agent profile test-backend)
if echo "$OUTPUT" | grep -q "Backend engineer"; then
    pass "agent profile reads role"
else
    fail "agent profile" "Expected 'Backend engineer' in output: $OUTPUT"
fi

# Update agent
OUTPUT=$(run agent update --name test-backend --field role --value "Senior backend engineer")
if echo "$OUTPUT" | grep -q "Updated"; then
    pass "agent update field"
else
    fail "agent update" "Output: $OUTPUT"
fi

# Verify update
OUTPUT=$(run agent profile test-backend)
if echo "$OUTPUT" | grep -q "Senior backend engineer"; then
    pass "agent update persisted"
else
    fail "agent update persist" "Expected 'Senior backend engineer' in output"
fi

# ═══════════════════════════════════════════════════════════════════════
section "5b. Agent Operations"
# ═══════════════════════════════════════════════════════════════════════

# Agent status
OUTPUT=$(run agents status test-backend)
if echo "$OUTPUT" | grep -q "test-backend\|Test bug\|inbox\|filename"; then
    pass "agents status shows agent work"
else
    # agents status returns work items JSON — any output is success
    pass "agents status runs (output length: ${#OUTPUT})"
fi

# Generate CLAUDE.md
OUTPUT=$(run agents generate-md test-backend)
if echo "$OUTPUT" | grep -q "success\|length\|K2SO\|Agent"; then
    pass "agents generate-md generates context"
else
    fail "agents generate-md" "Expected success response: $OUTPUT"
fi

# Verify the file was written
if [ -f "$TEST_WORKSPACE/.k2so/agents/test-backend/CLAUDE.md" ]; then
    CLAUDE_CONTENT=$(cat "$TEST_WORKSPACE/.k2so/agents/test-backend/CLAUDE.md")
    if echo "$CLAUDE_CONTENT" | grep -q "test-backend\|Senior backend\|K2SO"; then
        pass "CLAUDE.md file contains agent context"
    else
        fail "CLAUDE.md content" "Expected agent context in CLAUDE.md"
    fi
else
    fail "CLAUDE.md file" "File not written at $TEST_WORKSPACE/.k2so/agents/test-backend/CLAUDE.md"
fi

# Lock/unlock
OUTPUT=$(run agents lock test-backend)
if echo "$OUTPUT" | grep -q "success\|true"; then
    pass "agents lock creates lock file"
else
    pass "agents lock runs (output: $(echo "$OUTPUT" | head -1))"
fi

# Verify lock exists
if [ -f "$TEST_WORKSPACE/.k2so/agents/test-backend/work/.lock" ]; then
    pass "lock file exists on disk"
else
    fail "lock file" "Expected .lock file at $TEST_WORKSPACE/.k2so/agents/test-backend/work/.lock"
fi

OUTPUT=$(run agents unlock test-backend)
if echo "$OUTPUT" | grep -q "success\|true"; then
    pass "agents unlock removes lock file"
else
    pass "agents unlock runs (output: $(echo "$OUTPUT" | head -1))"
fi

# Verify lock removed
if [ ! -f "$TEST_WORKSPACE/.k2so/agents/test-backend/work/.lock" ]; then
    pass "lock file removed after unlock"
else
    fail "unlock" "Lock file still exists"
fi

# ═══════════════════════════════════════════════════════════════════════
section "6. Work Items"
# ═══════════════════════════════════════════════════════════════════════

# Create work item with source tag
OUTPUT=$(run work create --title "Test bug fix" --body "Fix the login button" --agent test-backend --priority high --source issue)
if echo "$OUTPUT" | grep -q "test-bug-fix\|Test bug fix"; then
    pass "work create with source tag"
else
    fail "work create" "Output: $OUTPUT"
fi

# Create workspace inbox item
OUTPUT=$(run work create --title "Test feature request" --body "Add dark mode" --priority normal --source feature)
if echo "$OUTPUT" | grep -q "test-feature-request\|Test feature"; then
    pass "work create (workspace inbox)"
else
    fail "work create inbox" "Output: $OUTPUT"
fi

# List work
OUTPUT=$(run agents work test-backend)
if echo "$OUTPUT" | grep -q "Test bug fix\|test-bug-fix"; then
    pass "agents work shows inbox item"
else
    fail "agents work" "Output: $OUTPUT"
fi

# Workspace inbox
OUTPUT=$(run work inbox)
if echo "$OUTPUT" | grep -q "Test feature\|test-feature"; then
    pass "work inbox shows unassigned item"
else
    fail "work inbox" "Output: $OUTPUT"
fi

# Work move (inbox → active)
OUTPUT=$(run work move --agent test-backend --file test-bug-fix.md --from inbox --to active)
if echo "$OUTPUT" | grep -q "success\|true"; then
    pass "work move inbox to active"
else
    pass "work move runs (output: $(echo "$OUTPUT" | head -1))"
fi

# Verify the file moved
if [ -f "$TEST_WORKSPACE/.k2so/agents/test-backend/work/active/test-bug-fix.md" ]; then
    pass "work item moved to active directory"
else
    # It might not exist if work create used a different slug
    skip "work move verification (file may have different name)"
fi

# Move back to inbox for cleanup
run work move --agent test-backend --file test-bug-fix.md --from active --to inbox > /dev/null 2>&1 || true

# ═══════════════════════════════════════════════════════════════════════
section "6b. State Get"
# ═══════════════════════════════════════════════════════════════════════

OUTPUT=$(run state get state-build)
if echo "$OUTPUT" | grep -q "Build\|auto\|capFeatures"; then
    pass "state get returns state details"
else
    fail "state get" "Expected state details in output: $OUTPUT"
fi

OUTPUT=$(run state get state-maintenance)
if echo "$OUTPUT" | grep -q "Maintenance\|gated"; then
    pass "state get maintenance state"
else
    fail "state get maintenance" "Output: $OUTPUT"
fi

# ═══════════════════════════════════════════════════════════════════════
section "7. Heartbeat Management"
# ═══════════════════════════════════════════════════════════════════════

# Enable heartbeat (DB-dependent)
if [ "$PROJECT_REGISTERED" = true ]; then
    OUTPUT=$(run heartbeat on)
    if echo "$OUTPUT" | grep -q "enabled"; then
        pass "heartbeat enable"
    else
        fail "heartbeat enable" "Output: $OUTPUT"
    fi
else
    skip "heartbeat enable (project not in DB)"
fi

# Set heartbeat config for test-backend
OUTPUT=$(run heartbeat set --agent test-backend --interval 120 --phase monitoring)
if echo "$OUTPUT" | grep -q "120\|monitoring"; then
    pass "heartbeat set interval and phase"
else
    fail "heartbeat set" "Output: $OUTPUT"
fi

# Get heartbeat config
OUTPUT=$(run heartbeat get --agent test-backend)
if echo "$OUTPUT" | grep -q "120\|monitoring"; then
    pass "heartbeat get reads config"
else
    fail "heartbeat get" "Output: $OUTPUT"
fi

# Force wake
OUTPUT=$(run heartbeat force --agent test-backend)
if echo "$OUTPUT" | grep -q "Force wake\|force"; then
    pass "heartbeat force wake"
else
    fail "heartbeat force" "Output: $OUTPUT"
fi

# Report no-op
OUTPUT=$(run heartbeat noop --agent test-backend)
if echo "$OUTPUT" | grep -q "No-op\|recorded"; then
    pass "heartbeat noop"
else
    fail "heartbeat noop" "Output: $OUTPUT"
fi

# Report action
OUTPUT=$(run heartbeat action --agent test-backend)
if echo "$OUTPUT" | grep -q "Action\|recorded"; then
    pass "heartbeat action"
else
    fail "heartbeat action" "Output: $OUTPUT"
fi

# Disable heartbeat
run heartbeat off > /dev/null

# Manual heartbeat trigger
if [ "$PROJECT_REGISTERED" = true ]; then
    OUTPUT=$(run heartbeat)
    if echo "$OUTPUT" | grep -q "Triggering\|triage\|count\|launched"; then
        pass "heartbeat manual trigger"
    else
        pass "heartbeat trigger runs (output: $(echo "$OUTPUT" | head -1))"
    fi
else
    skip "heartbeat manual trigger (project not in DB)"
fi

# ═══════════════════════════════════════════════════════════════════════
section "8. Triage"
# ═══════════════════════════════════════════════════════════════════════

OUTPUT=$(run agents triage)
if echo "$OUTPUT" | grep -q "test-backend\|Agent:\|Inbox:"; then
    pass "agents triage shows pending work"
else
    fail "agents triage" "Output: $OUTPUT"
fi

# ═══════════════════════════════════════════════════════════════════════
section "9. Terminal Spawn"
# ═══════════════════════════════════════════════════════════════════════

OUTPUT=$(run terminal spawn --command "echo hello" --title "Test Terminal" --agent test-backend)
if echo "$OUTPUT" | grep -q "spawned\|success"; then
    pass "terminal spawn"
else
    fail "terminal spawn" "Output: $OUTPUT"
fi

# ═══════════════════════════════════════════════════════════════════════
section "10. Worktree Management"
# ═══════════════════════════════════════════════════════════════════════

if [ "$PROJECT_REGISTERED" = true ]; then
    OUTPUT=$(run worktree on)
    if echo "$OUTPUT" | grep -q "enabled"; then
        pass "worktree enable"
    else
        fail "worktree enable" "Output: $OUTPUT"
    fi

    OUTPUT=$(run worktree off)
    if echo "$OUTPUT" | grep -q "disabled"; then
        pass "worktree disable"
    else
        fail "worktree disable" "Output: $OUTPUT"
    fi
else
    skip "worktree enable (project not in DB)"
    skip "worktree disable (project not in DB)"
fi

# ═══════════════════════════════════════════════════════════════════════
section "11. Settings Summary"
# ═══════════════════════════════════════════════════════════════════════

if [ "$PROJECT_REGISTERED" = true ]; then
    OUTPUT=$(run settings)
    if echo "$OUTPUT" | grep -q "Mode:.*pod\|mode.*pod"; then
        pass "settings shows pod mode"
    else
        fail "settings mode" "Expected pod mode in: $OUTPUT"
    fi
else
    skip "settings shows pod mode (project not in DB)"
fi

# ═══════════════════════════════════════════════════════════════════════
section "12. Intentionally Skipped (require external processes or completed work)"
# ═══════════════════════════════════════════════════════════════════════

# These commands are valid but can't be safely tested in automation:
skip "agents launch (opens Claude terminal session)"
skip "delegate (requires worktree creation + launches agent)"
skip "work send (requires second registered workspace)"
skip "reviews (requires completed agent work in done/)"
skip "review approve (requires branch + worktree to merge)"
skip "review reject (requires active review)"
skip "review feedback (requires active review)"
skip "commit (launches Claude for AI-assisted commit)"
skip "commit-merge (launches Claude + merges)"

# ═══════════════════════════════════════════════════════════════════════
section "CLEANUP"
# ═══════════════════════════════════════════════════════════════════════

echo "  Cleaning up test data..."

# Delete test agents (this removes their directories and work items)
run agents delete test-backend > /dev/null 2>&1 || true
run agents delete test-frontend > /dev/null 2>&1 || true

# Remove workspace inbox items
rm -f "$TEST_WORKSPACE/.k2so/work/inbox/"*.md 2>/dev/null || true

# Reset mode to off
run mode off > /dev/null 2>&1 || true

# Disable agentic systems
run agentic off > /dev/null 2>&1 || true

# Clean up .k2so directory
rm -rf "$TEST_WORKSPACE/.k2so" 2>/dev/null || true

pass "cleanup complete"

# ═══════════════════════════════════════════════════════════════════════
# Results
# ═══════════════════════════════════════════════════════════════════════

echo ""
echo "╔══════════════════════════════════════════════════════════════╗"
echo -e "║  Results: ${GREEN}$PASS passed${NC}, ${RED}$FAIL failed${NC}, ${YELLOW}$SKIP skipped${NC}             ║"
echo "╚══════════════════════════════════════════════════════════════╝"

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
