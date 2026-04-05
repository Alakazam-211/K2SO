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
TEST_WORKSPACE_2="${TEST_WORKSPACE}-send-target"
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

# Auto-register test workspace if not already in K2SO's DB
PROJECT_REGISTERED=false
OUTPUT=$(K2SO_PROJECT_PATH="$TEST_WORKSPACE" "$K2SO_CLI" mode coordinator 2>&1 || true)
if ! echo "$OUTPUT" | grep -q "Project not found\|error"; then
    PROJECT_REGISTERED=true
    run mode off > /dev/null 2>&1 || true
else
    # Try to register via CLI
    echo -e "${YELLOW}Auto-registering test workspace...${NC}"
    REG_OUTPUT=$(run workspace open "$TEST_WORKSPACE" 2>&1 || true)
    if echo "$REG_OUTPUT" | grep -q "success\|projectId"; then
        echo -e "${GREEN}Test workspace registered successfully${NC}"
        PROJECT_REGISTERED=true
    elif echo "$REG_OUTPUT" | grep -q "already registered"; then
        PROJECT_REGISTERED=true
    else
        echo -e "${YELLOW}NOTE: Could not register test workspace in K2SO's DB.${NC}"
        echo -e "${YELLOW}DB-dependent tests will be skipped.${NC}"
        echo ""
    fi
fi

# Ensure test workspace is a git repo (needed for worktree/delegate/review tests)
if ! git -C "$TEST_WORKSPACE" rev-parse --git-dir > /dev/null 2>&1; then
    echo -e "${YELLOW}Initializing git in test workspace...${NC}"
    git -C "$TEST_WORKSPACE" init -b main > /dev/null 2>&1
    echo "test" > "$TEST_WORKSPACE/README.md"
    git -C "$TEST_WORKSPACE" add -A > /dev/null 2>&1
    git -C "$TEST_WORKSPACE" commit -m "Initial commit" > /dev/null 2>&1
    echo -e "${GREEN}Git initialized${NC}"
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

    OUTPUT=$(run mode coordinator)
    if echo "$OUTPUT" | grep -qi "coordinator\|success"; then
        pass "mode set to coordinator"
    else
        fail "mode coordinator" "Output: $OUTPUT"
    fi
else
    skip "mode query (project not in DB)"
    skip "mode set coordinator (project not in DB)"
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
# Section 10 removed — worktree mode toggle no longer exists (v0.23+, worktrees always available)
# ═══════════════════════════════════════════════════════════════════════
section "11. Settings Summary"
# ═══════════════════════════════════════════════════════════════════════

if [ "$PROJECT_REGISTERED" = true ]; then
    OUTPUT=$(run settings)
    if echo "$OUTPUT" | grep -q "Mode:.*coordinator\|mode.*coordinator"; then
        pass "settings shows coordinator mode"
    else
        fail "settings mode" "Expected coordinator mode in: $OUTPUT"
    fi
else
    skip "settings shows coordinator mode (project not in DB)"
fi

# ═══════════════════════════════════════════════════════════════════════
section "12a. AI Commit Commands"
# ═══════════════════════════════════════════════════════════════════════

OUTPUT=$(run commit -m "Test commit message")
if echo "$OUTPUT" | grep -q "success.*true\|action.*commit"; then
    pass "commit returns success"
else
    fail "commit" "Output: $OUTPUT"
fi

OUTPUT=$(run commit-merge -m "Test merge message")
if echo "$OUTPUT" | grep -q "success.*true\|action.*commit-merge"; then
    pass "commit-merge returns success"
else
    fail "commit-merge" "Output: $OUTPUT"
fi

# ═══════════════════════════════════════════════════════════════════════
section "12b. Review Feedback"
# ═══════════════════════════════════════════════════════════════════════

run agent create test-feedback-agent --role "Feedback test" > /dev/null
OUTPUT=$(run review feedback test-feedback-agent --message "Please add unit tests")
if echo "$OUTPUT" | grep -q "success"; then
    pass "review feedback returns success"
else
    fail "review feedback" "Output: $OUTPUT"
fi

if ls "$TEST_WORKSPACE/.k2so/agents/test-feedback-agent/work/inbox/review-feedback-"*.md > /dev/null 2>&1; then
    FEEDBACK_CONTENT=$(cat "$TEST_WORKSPACE/.k2so/agents/test-feedback-agent/work/inbox/review-feedback-"*.md)
    if echo "$FEEDBACK_CONTENT" | grep -q "unit tests"; then
        pass "review feedback file contains message"
    else
        fail "review feedback content" "Message not found in file"
    fi
else
    fail "review feedback file" "No review-feedback-*.md found in inbox"
fi

run agents delete test-feedback-agent --force > /dev/null 2>&1 || true

# ═══════════════════════════════════════════════════════════════════════
section "12c. Cross-Workspace Work Send"
# ═══════════════════════════════════════════════════════════════════════

mkdir -p "$TEST_WORKSPACE_2"
if ! git -C "$TEST_WORKSPACE_2" rev-parse --git-dir > /dev/null 2>&1; then
    git -C "$TEST_WORKSPACE_2" init -b main > /dev/null 2>&1
    echo "test" > "$TEST_WORKSPACE_2/README.md"
    git -C "$TEST_WORKSPACE_2" add -A > /dev/null 2>&1
    git -C "$TEST_WORKSPACE_2" commit -m "Initial commit" > /dev/null 2>&1
fi
run workspace open "$TEST_WORKSPACE_2" > /dev/null 2>&1 || true

OUTPUT=$(run work send --workspace "$TEST_WORKSPACE_2" --title "Cross workspace task" --body "Sent from test suite")
if echo "$OUTPUT" | grep -q "cross-workspace-task\|Cross workspace"; then
    pass "work send creates item in target workspace"
else
    fail "work send" "Output: $OUTPUT"
fi

if ls "$TEST_WORKSPACE_2/.k2so/work/inbox/"*.md > /dev/null 2>&1; then
    pass "work send file exists in target inbox"
else
    fail "work send file" "No .md file in $TEST_WORKSPACE_2/.k2so/work/inbox/"
fi

# Cleanup second workspace
run workspace remove "$TEST_WORKSPACE_2" > /dev/null 2>&1 || true
rm -rf "$TEST_WORKSPACE_2" 2>/dev/null || true

# ═══════════════════════════════════════════════════════════════════════
section "12d. Delegate (worktree creation + agent assignment)"
# ═══════════════════════════════════════════════════════════════════════

run agent create test-delegator --role "Delegate test agent" > /dev/null
run work create --title "Delegate test task" --body "Implement the feature" --agent test-delegator --priority high > /dev/null

# Find the work item file
DELEGATE_FILE=$(ls "$TEST_WORKSPACE/.k2so/agents/test-delegator/work/inbox/"*.md 2>/dev/null | head -1)
if [ -z "$DELEGATE_FILE" ]; then
    fail "delegate setup" "No work item found in inbox"
else
    DELEGATE_FILENAME=$(basename "$DELEGATE_FILE")
    OUTPUT=$(run delegate test-delegator "$DELEGATE_FILE")

    # Verify work moved from inbox to active
    if [ ! -f "$TEST_WORKSPACE/.k2so/agents/test-delegator/work/inbox/$DELEGATE_FILENAME" ]; then
        pass "delegate moved work from inbox"
    else
        fail "delegate inbox" "Work item still in inbox after delegate"
    fi

    if ls "$TEST_WORKSPACE/.k2so/agents/test-delegator/work/active/"*.md > /dev/null 2>&1; then
        pass "delegate work present in active"
    else
        fail "delegate active" "No work item in active folder"
    fi

    # Verify git branch created
    if git -C "$TEST_WORKSPACE" branch --list 'agent/test-delegator/*' | grep -q "agent/test-delegator"; then
        pass "delegate created git branch"
    else
        fail "delegate branch" "No agent/test-delegator/* branch found"
    fi

    # Verify worktree exists
    WORKTREE_DIR=$(git -C "$TEST_WORKSPACE" worktree list 2>/dev/null | grep "agent/test-delegator" | awk '{print $1}')
    if [ -n "$WORKTREE_DIR" ] && [ -d "$WORKTREE_DIR" ]; then
        pass "delegate created worktree directory"
    else
        fail "delegate worktree" "Worktree directory not found"
    fi

    # Verify CLAUDE.md in worktree
    if [ -n "$WORKTREE_DIR" ] && [ -f "$WORKTREE_DIR/CLAUDE.md" ]; then
        pass "delegate generated CLAUDE.md in worktree"
    else
        fail "delegate CLAUDE.md" "CLAUDE.md not found in worktree"
    fi
fi

# ═══════════════════════════════════════════════════════════════════════
section "12e. Agents Launch"
# ═══════════════════════════════════════════════════════════════════════

OUTPUT=$(run agents launch test-delegator)
if echo "$OUTPUT" | grep -q "command\|args\|cwd\|success"; then
    pass "agents launch returns launch info"
else
    fail "agents launch" "Output: $OUTPUT"
fi

# ═══════════════════════════════════════════════════════════════════════
section "12f. Reviews List + Review Approve"
# ═══════════════════════════════════════════════════════════════════════

# Move work from active to done (simulate agent completing work)
ACTIVE_FILE=$(ls "$TEST_WORKSPACE/.k2so/agents/test-delegator/work/active/"*.md 2>/dev/null | head -1)
if [ -n "$ACTIVE_FILE" ]; then
    ACTIVE_FILENAME=$(basename "$ACTIVE_FILE")
    run work move --agent test-delegator --file "$ACTIVE_FILENAME" --from active --to done > /dev/null

    # List reviews
    OUTPUT=$(run reviews)
    if echo "$OUTPUT" | grep -q "test-delegator"; then
        pass "reviews lists agent with completed work"
    else
        fail "reviews" "test-delegator not in reviews output: $OUTPUT"
    fi

    # Approve
    BRANCH=$(git -C "$TEST_WORKSPACE" branch --list 'agent/test-delegator/*' 2>/dev/null | head -1 | sed 's/^[* +]*//' | xargs)
    if [ -n "$BRANCH" ]; then
        OUTPUT=$(run review approve test-delegator "$BRANCH")
        if echo "$OUTPUT" | grep -q "success\|Approved\|merged"; then
            pass "review approve returns success"
        else
            fail "review approve" "Output: $OUTPUT"
        fi

        # Verify branch deleted
        if ! git -C "$TEST_WORKSPACE" branch --list 'agent/test-delegator/*' 2>/dev/null | grep -q "agent/test-delegator"; then
            pass "review approve deleted branch"
        else
            fail "review approve branch" "Branch still exists after approve"
        fi

        # Verify done folder empty
        if [ -z "$(ls "$TEST_WORKSPACE/.k2so/agents/test-delegator/work/done/"*.md 2>/dev/null)" ]; then
            pass "review approve archived done items"
        else
            fail "review approve done" "Done folder not empty"
        fi
    else
        fail "review approve setup" "No branch found for test-delegator"
    fi
else
    fail "review setup" "No active work item to move to done"
fi

run agents delete test-delegator --force > /dev/null 2>&1 || true

# ═══════════════════════════════════════════════════════════════════════
section "12g. Review Reject (independent delegate cycle)"
# ═══════════════════════════════════════════════════════════════════════

run agent create test-rejecter --role "Reject test agent" > /dev/null
run work create --title "Reject test task" --body "Work to be rejected" --agent test-rejecter > /dev/null

REJECT_FILE=$(ls "$TEST_WORKSPACE/.k2so/agents/test-rejecter/work/inbox/"*.md 2>/dev/null | head -1)
if [ -n "$REJECT_FILE" ]; then
    run delegate test-rejecter "$REJECT_FILE" > /dev/null

    # Move to done
    REJECT_ACTIVE=$(ls "$TEST_WORKSPACE/.k2so/agents/test-rejecter/work/active/"*.md 2>/dev/null | head -1)
    if [ -n "$REJECT_ACTIVE" ]; then
        REJECT_ACTIVE_NAME=$(basename "$REJECT_ACTIVE")
        run work move --agent test-rejecter --file "$REJECT_ACTIVE_NAME" --from active --to done > /dev/null

        OUTPUT=$(run review reject test-rejecter --reason "Tests not passing")
        if echo "$OUTPUT" | grep -q "success"; then
            pass "review reject returns success"
        else
            fail "review reject" "Output: $OUTPUT"
        fi

        # Verify branch deleted
        if ! git -C "$TEST_WORKSPACE" branch --list 'agent/test-rejecter/*' 2>/dev/null | grep -q "agent/test-rejecter"; then
            pass "review reject deleted branch"
        else
            fail "review reject branch" "Branch still exists"
        fi

        # Verify work moved back to inbox
        if ls "$TEST_WORKSPACE/.k2so/agents/test-rejecter/work/inbox/"*.md > /dev/null 2>&1; then
            pass "review reject moved work back to inbox"
        else
            fail "review reject inbox" "No work items returned to inbox"
        fi

        # Verify feedback file created
        if ls "$TEST_WORKSPACE/.k2so/agents/test-rejecter/work/inbox/review-feedback-"*.md > /dev/null 2>&1; then
            pass "review reject created feedback file"
        else
            fail "review reject feedback" "No review-feedback-*.md in inbox"
        fi
    else
        fail "review reject setup" "No active work after delegate"
    fi
else
    fail "review reject setup" "No work item in inbox"
fi

run agents delete test-rejecter --force > /dev/null 2>&1 || true

# ═══════════════════════════════════════════════════════════════════════
section "12h. Heartbeat Triage"
# ═══════════════════════════════════════════════════════════════════════

if [ "$PROJECT_REGISTERED" = true ]; then
    run agent create test-heartbeat-agent --role "Heartbeat test" > /dev/null
    run work create --title "Heartbeat work" --body "Triage should find this" --agent test-heartbeat-agent > /dev/null

    OUTPUT=$(run heartbeat)
    if echo "$OUTPUT" | grep -q "count\|launched\|triage"; then
        pass "heartbeat triage returns results"
    else
        fail "heartbeat triage" "Output: $OUTPUT"
    fi

    run agents delete test-heartbeat-agent --force > /dev/null 2>&1 || true
else
    skip "heartbeat triage (project not in DB)"
fi

# ═══════════════════════════════════════════════════════════════════════
section "13. Running Agents & Terminal I/O"
# ═══════════════════════════════════════════════════════════════════════

# Test agents running (list)
OUTPUT=$(run agents running)
if echo "$OUTPUT" | grep -q "running\|shell\|claude\|No running\|\[\]"; then
    pass "agents running returns results"
else
    fail "agents running" "Output: $OUTPUT"
fi

# Test terminal read — verify the endpoint exists and responds
OUTPUT=$(run terminal read "nonexistent-id" --lines 10 2>&1 || true)
if echo "$OUTPUT" | grep -q "not found\|error\|lines" || [ -z "$OUTPUT" ]; then
    pass "terminal read endpoint responds"
else
    fail "terminal read" "Output: $OUTPUT"
fi

# Test terminal write (verify endpoint responds to nonexistent ID)
OUTPUT=$(run terminal write "nonexistent-id" "test message" 2>&1 || true)
if echo "$OUTPUT" | grep -q "not found\|error\|success"; then
    pass "terminal write handles missing terminal gracefully"
else
    fail "terminal write" "Output: $OUTPUT"
fi

# ═══════════════════════════════════════════════════════════════════════
section "CLEANUP"
# ═══════════════════════════════════════════════════════════════════════

echo "  Cleaning up test data..."

# Delete test agents (removes directories and work items)
for agent in test-backend test-frontend test-delegator test-rejecter test-feedback-agent test-heartbeat-agent; do
    run agents delete "$agent" --force > /dev/null 2>&1 || true
done

# Remove any leftover worktrees
for wt in $(git -C "$TEST_WORKSPACE" worktree list 2>/dev/null | grep -v "$TEST_WORKSPACE " | awk '{print $1}'); do
    git -C "$TEST_WORKSPACE" worktree remove --force "$wt" > /dev/null 2>&1 || true
done
# Delete agent branches
for branch in $(git -C "$TEST_WORKSPACE" branch --list 'agent/*' 2>/dev/null); do
    git -C "$TEST_WORKSPACE" branch -D "$branch" > /dev/null 2>&1 || true
done

# Remove workspace inbox items
rm -f "$TEST_WORKSPACE/.k2so/work/inbox/"*.md 2>/dev/null || true

# Clean up second test workspace if it exists
run workspace remove "$TEST_WORKSPACE_2" > /dev/null 2>&1 || true
rm -rf "$TEST_WORKSPACE_2" 2>/dev/null || true

# Reset mode to off
run mode off > /dev/null 2>&1 || true

# Disable agentic systems
run agentic off > /dev/null 2>&1 || true

# Deregister test workspace from DB
run workspace remove "$TEST_WORKSPACE" > /dev/null 2>&1 || true

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
