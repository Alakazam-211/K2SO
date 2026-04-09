#!/bin/bash
# K2SO Companion WebSocket Protocol Test Script
# Tests all WS methods against a running K2SO instance.
#
# Prerequisites:
#   - K2SO running with companion enabled
#   - websocat installed (brew install websocat)
#   - Companion credentials configured
#
# Usage:
#   ./scripts/test-companion-ws.sh <url> <username> <password>
#   ./scripts/test-companion-ws.sh https://k2.ngrok.app z3thon mr0ss0nl

set -euo pipefail

URL="${1:-}"
USERNAME="${2:-}"
PASSWORD="${3:-}"

if [ -z "$URL" ] || [ -z "$USERNAME" ] || [ -z "$PASSWORD" ]; then
    echo "Usage: ./scripts/test-companion-ws.sh <url> <username> <password>" >&2
    exit 1
fi

URL="${URL%/}"
WS_URL=$(echo "$URL" | sed 's|^https://|wss://|; s|^http://|ws://|')

PASS=0
FAIL=0

GREEN='\033[0;32m'
RED='\033[0;31m'
CYAN='\033[0;36m'
NC='\033[0m'

passed() { ((PASS++)); echo -e "  ${GREEN}✓ PASS${NC}"; }
failed() { ((FAIL++)); echo -e "  ${RED}✗ FAIL: $1${NC}"; }

# Get auth token via HTTP (same as mobile app login flow)
echo -e "${CYAN}═══════════════════════════════════════════════════${NC}"
echo -e "${CYAN}  K2SO Companion WebSocket Test Suite${NC}"
echo -e "${CYAN}  Target: $WS_URL${NC}"
echo -e "${CYAN}═══════════════════════════════════════════════════${NC}"
echo ""

echo -e "${CYAN}0. HTTP Auth (get token for WS)${NC}"
AUTH_RESP=$(curl -s --max-time 10 -X POST \
    -H "Authorization: Basic $(echo -n "$USERNAME:$PASSWORD" | base64)" \
    -H "ngrok-skip-browser-warning: true" \
    "$URL/companion/auth")
TOKEN=$(echo "$AUTH_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['token'])" 2>/dev/null || echo "")

if [ -z "$TOKEN" ]; then
    echo -e "  ${RED}✗ Failed to get auth token. Is the companion running?${NC}"
    echo "  Response: $AUTH_RESP"
    exit 1
fi
echo -e "  Token: ${TOKEN:0:16}..."
echo ""

# Helper: send a WS method and capture response
ws_call() {
    local method="$1"
    local params="$2"
    local id="$RANDOM"
    local msg="{\"id\":\"$id\",\"method\":\"$method\",\"params\":$params}"
    # Send message, read responses until we get one with our ID
    echo "$msg" | timeout 10 websocat -1 -B 1048576 "$WS_URL/companion/ws?token=$TOKEN" 2>/dev/null || echo ""
}

# Helper: check if response has our result
check_result() {
    local resp="$1"
    local check="$2"
    if echo "$resp" | python3 -c "
import sys,json
for line in sys.stdin:
    line = line.strip()
    if not line: continue
    try:
        d = json.loads(line)
        if 'result' in d:
            $check
            sys.exit(0)
    except SystemExit: raise
    except: pass
sys.exit(1)
" 2>/dev/null; then
        return 0
    else
        return 1
    fi
}

# ── 1. Ping ──────────────────────────────────────────────────────────────
echo -e "${CYAN}1. method: ping${NC}"
RESP=$(ws_call "ping" "{}")
if echo "$RESP" | grep -q '"pong"'; then
    passed
else
    failed "Expected pong response"
    echo "   Response: $(echo "$RESP" | head -1 | cut -c1-100)"
fi
echo ""

# ── 2. Projects List (global) ───────────────────────────────────────────
echo -e "${CYAN}2. method: projects.list${NC}"
RESP=$(ws_call "projects.list" "{}")
COUNT=$(echo "$RESP" | python3 -c "
import sys,json
for line in sys.stdin:
    try:
        d = json.loads(line.strip())
        if 'result' in d and 'data' in d['result']:
            data = d['result']['data']
            if isinstance(data, list): print(len(data)); sys.exit(0)
    except: pass
print(0)
" 2>/dev/null || echo "0")
if [ "$COUNT" -gt 0 ] 2>/dev/null; then
    passed
    echo "   Found $COUNT workspace(s)"
else
    failed "Expected projects list"
    echo "   Response: $(echo "$RESP" | head -1 | cut -c1-100)"
fi
echo ""

# ── 3. Projects Summary (global) ────────────────────────────────────────
echo -e "${CYAN}3. method: projects.summary${NC}"
RESP=$(ws_call "projects.summary" "{}")
if echo "$RESP" | python3 -c "
import sys,json
for line in sys.stdin:
    try:
        d = json.loads(line.strip())
        if 'result' in d and 'data' in d['result']: print('ok'); sys.exit(0)
    except: pass
sys.exit(1)
" 2>/dev/null; then
    passed
else
    failed "Expected summary data"
fi
echo ""

# ── 4. Sessions List (global) ───────────────────────────────────────────
echo -e "${CYAN}4. method: sessions.list${NC}"
RESP=$(ws_call "sessions.list" "{}")
if echo "$RESP" | python3 -c "
import sys,json
for line in sys.stdin:
    try:
        d = json.loads(line.strip())
        if 'result' in d: print('ok'); sys.exit(0)
    except: pass
sys.exit(1)
" 2>/dev/null; then
    passed
else
    failed "Expected sessions data"
fi
echo ""

# Get first project path for scoped tests
PROJECT_PATH=$(ws_call "projects.list" "{}" | python3 -c "
import sys,json
for line in sys.stdin:
    try:
        d = json.loads(line.strip())
        if 'result' in d and 'data' in d['result']:
            for p in d['result']['data']:
                if isinstance(p, dict) and 'path' in p:
                    print(p['path']); sys.exit(0)
    except: pass
" 2>/dev/null || echo "")

if [ -z "$PROJECT_PATH" ]; then
    echo -e "${RED}Could not get project path — skipping scoped tests${NC}"
else
    PROJ_JSON=$(python3 -c "import json; print(json.dumps('$PROJECT_PATH'))")
    PARAMS="{\"project\":$PROJ_JSON}"

    # ── 5. Agents List ───────────────────────────────────────────────────
    echo -e "${CYAN}5. method: agents.list${NC}"
    RESP=$(ws_call "agents.list" "$PARAMS")
    if echo "$RESP" | python3 -c "
import sys,json
for line in sys.stdin:
    try:
        d = json.loads(line.strip())
        if 'result' in d: print('ok'); sys.exit(0)
    except: pass
sys.exit(1)
" 2>/dev/null; then
        passed
    else
        failed "Expected agents data"
    fi
    echo ""

    # ── 6. Agents Running ────────────────────────────────────────────────
    echo -e "${CYAN}6. method: agents.running${NC}"
    RESP=$(ws_call "agents.running" "$PARAMS")
    if echo "$RESP" | python3 -c "
import sys,json
for line in sys.stdin:
    try:
        d = json.loads(line.strip())
        if 'result' in d: print('ok'); sys.exit(0)
    except: pass
sys.exit(1)
" 2>/dev/null; then
        passed
    else
        failed "Expected running agents data"
    fi
    echo ""

    # ── 7. Status ────────────────────────────────────────────────────────
    echo -e "${CYAN}7. method: status${NC}"
    RESP=$(ws_call "status" "$PARAMS")
    if echo "$RESP" | python3 -c "
import sys,json
for line in sys.stdin:
    try:
        d = json.loads(line.strip())
        if 'result' in d: print('ok'); sys.exit(0)
    except: pass
sys.exit(1)
" 2>/dev/null; then
        passed
    else
        failed "Expected status data"
    fi
    echo ""

    # ── 8. Reviews List ──────────────────────────────────────────────────
    echo -e "${CYAN}8. method: reviews.list${NC}"
    RESP=$(ws_call "reviews.list" "$PARAMS")
    if echo "$RESP" | python3 -c "
import sys,json
for line in sys.stdin:
    try:
        d = json.loads(line.strip())
        if 'result' in d: print('ok'); sys.exit(0)
    except: pass
sys.exit(1)
" 2>/dev/null; then
        passed
    else
        failed "Expected reviews data"
    fi
    echo ""
fi

# ── 9. Auth via WS ──────────────────────────────────────────────────────
echo -e "${CYAN}9. method: auth (over WS)${NC}"
# Open WS without token, then auth as first message
AUTH_MSG="{\"id\":\"auth1\",\"method\":\"auth\",\"params\":{\"token\":\"$TOKEN\"}}"
RESP=$(echo "$AUTH_MSG" | timeout 10 websocat -1 -B 65536 "$WS_URL/companion/ws" 2>/dev/null || echo "")
if echo "$RESP" | grep -q '"authenticated"'; then
    passed
else
    failed "Expected authenticated:true"
    echo "   Response: $(echo "$RESP" | head -1 | cut -c1-100)"
fi
echo ""

# ── 10. Unauthenticated request ─────────────────────────────────────────
echo -e "${CYAN}10. Unauthenticated WS request (should fail)${NC}"
RESP=$(echo '{"id":"x","method":"projects.list","params":{}}' | timeout 10 websocat -1 -B 65536 "$WS_URL/companion/ws" 2>/dev/null || echo "")
if echo "$RESP" | grep -q '"error"'; then
    passed
else
    failed "Expected error for unauthenticated request"
    echo "   Response: $(echo "$RESP" | head -1 | cut -c1-100)"
fi
echo ""

# ── 11. Unknown method ──────────────────────────────────────────────────
echo -e "${CYAN}11. Unknown method (should return error)${NC}"
RESP=$(ws_call "nonexistent.method" "{}")
if echo "$RESP" | grep -q '"error"'; then
    passed
else
    failed "Expected error for unknown method"
    echo "   Response: $(echo "$RESP" | head -1 | cut -c1-100)"
fi
echo ""

# ── 12. Terminal subscribe ───────────────────────────────────────────────
echo -e "${CYAN}12. method: terminal.subscribe${NC}"
RESP=$(ws_call "terminal.subscribe" "{\"terminalId\":\"test-terminal\"}")
if echo "$RESP" | grep -q '"subscribed"'; then
    passed
else
    failed "Expected subscribed confirmation"
    echo "   Response: $(echo "$RESP" | head -1 | cut -c1-100)"
fi
echo ""

# ── Summary ──────────────────────────────────────────────────────────────
echo -e "${CYAN}═══════════════════════════════════════════════════${NC}"
echo -e "  Results: ${GREEN}$PASS passed${NC}, ${RED}$FAIL failed${NC}"
echo -e "${CYAN}═══════════════════════════════════════════════════${NC}"

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
