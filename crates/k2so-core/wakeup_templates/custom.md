<!-- DEFAULT TEMPLATE — K2SO scaffolded this for a custom agent.
     Edit below to customize what this agent does when the heartbeat wakes it.
     Delete this comment once you've made it your own. -->

# On wake-up — Custom Agent

1. Run `k2so checkin --agent <your-name>` to see your state: the task you were working on (if any), your inbox, messages from peers, and recent activity.
2. If you were mid-task (something in `active/`), resume it. Pick up where you left off — your prior session transcript is your best context.
3. If you have new items in `inbox/`, start on the highest priority one. Move it to `active/` via `k2so work move` before beginning.
4. If there's nothing to do, run `k2so done` or `k2so noop` (if you want to trigger auto-backoff) and exit.

Check for messages from peers every time you wake — especially `--wake` messages, which are urgent.
