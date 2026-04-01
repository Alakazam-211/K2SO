---
title: Bug: heartbeat.port file missing â automatic heartbeat triage never fires
priority: high
assigned_by: user
created: 2026-03-30
type: task
source: issue
---

## Summary

The macOS LaunchAgent (com.k2so.agent-heartbeat) is installed and running every 300s, but the heartbeat script silently exits because ~/.k2so/heartbeat.port does not exist. This means automatic triage never fires for any heartbeat-enabled workspace.

## Impact

The Peliguard/SarahAI workspace has 5 bug items sitting in its inbox that have never been picked up by the pod leader. Manual `k2so heartbeat` works fine â only the automatic background cycle is broken.

## Root Cause

The K2SO Tauri app (PID running) is not writing `~/.k2so/heartbeat.port` on startup. The heartbeat.sh script checks for this file early and exits 0 if missing:

```bash
if [ ! -f "$PORT_FILE" ]; then
    exit 0
fi
```

So the script never reaches the curl call to `/cli/scheduler-tick`.

## Evidence

- `launchctl list | grep k2so` shows the LaunchAgent is loaded
- `~/.k2so/heartbeat-projects.txt` correctly lists `/Users/z3thon/DevProjects/Peliguard/SarahAI`
- `~/.k2so/heartbeat.token` exists
- `~/.k2so/heartbeat.port` â FILE NOT FOUND
- `~/.k2so/heartbeat.log` â FILE NOT FOUND (script never gets far enough to log)
- `~/.k2so/heartbeat-stderr.log` â empty
- Manual `k2so heartbeat` succeeds and launches `__lead__` correctly

## Expected Fix

The K2SO server should write `~/.k2so/heartbeat.port` on startup (or when the HTTP listener binds). Verify it persists across app restarts and is cleaned up on shutdown.
