# Multi-User Presence & Claiming — Ideas Parking Lot

**Status:** PARKING LOT (2026-06-10) — ideas to revisit, NOT a build plan.
**Decision of record (Rosson):** the current claiming system stays as-is for
now — last-typist-wins + explicit focus claims (0.39.43), with the shared grid
emitter (0.39.46) feeding every viewer correctly. This doc captures everything
we brainstormed beyond that, so nothing is lost when we revisit.
**Related:** `.k2so/prds/0.39.x-final-creature-comforts.md` §1 (presence chip)
and §2.5 (size claim & auto-fit — the subset already scoped for the final
0.39.x release).

---

## 1. Room presence — "you know who's in the room with you"

The mental model: opening a workspace/tab is **entering a room**. Others in
that room should see you arrive, and you should know when the interactive
space you're using is shared. This extends §1's server-level presence chip
down through the navigation hierarchy.

### Where presence would surface (most → least ambient)

| Surface | What it shows | Signal |
|---|---|---|
| Top bar chip (§1, scoped) | total users on this server | "the building has people in it" |
| **Workspace icon (rail + sidebar)** | small count/avatar badge | "someone is in this room" |
| **Tab strip** | per-tab viewer dot/initials | "someone is looking at this exact tab" |
| Terminal pane header (§1, scoped) | "2 watching" on the session | "someone is watching this terminal" |
| **Shared-space emphasis** | when YOUR focused tab has other viewers, the presence indicator warms up (color/"with baden") | "you are sharing this interactive space *right now*" |

The last row is the one that matters most — it's the difference between a
dashboard statistic and the social awareness Rosson described.

### Two data tiers (build passive first)

**Tier 1 — passive, derivable today (no new client reporting):**
- Per-session viewer counts already exist (`subscriber_count`, grid-WS attach).
- Session → workspace rollup via the session's cwd gives **workspace-level**
  counts for free: "N viewers attached to terminals in this workspace."
- Honest limitation: attachment ≠ looking. A backgrounded pane counts.

**Tier 2 — active focus sharing (opt-in, new):**
- Clients publish their focused workspace + tab to the presence registry
  ("I'm in this room"). This is per-client view state, which 0.39.42
  deliberately made private — so sharing it must be **explicit and opt-in**
  (a "share my focus" toggle riding the presence settings).
- Unlocks: per-TAB presence, avatars/initials on tabs, the shared-space
  emphasis, and (later) follow-the-user (which we already deferred once).
- Wire sketch: extend `presence_changed` viewers with optional
  `{ focusedWorkspace?: path, focusedTab?: id }` — additive, wire-compatible.

### Display nuances captured
- Counts at low zoom, initials/avatars on hover; never names-by-default on
  tiny surfaces (clutter).
- Dedupe per user (×N connections, one avatar).
- Viewer-role users show with the 👁 affix so "watching" vs "can type" reads
  at a glance.
- Decay/ghosting: a focus report older than ~30s without a heartbeat fades —
  no stale "baden is here" after a sleep-closed laptop.

---

## 2. Claiming — current state (what we're keeping)

- **Typing claims instantly** (last-typist-wins, 0.39.43 §6.4) and snaps the
  PTY to the typist's dims.
- **Focus claims** (`SetActive` + dims) on window focus.
- Scroll never claims (client-local in v2). Composer sends (future, §4 of the
  creature-comforts PRD) will never claim.
- One PTY size is settled physics: TIOCSWINSZ is kernel truth, a TUI renders
  for exactly one width; per-viewer sizes would require tmux-style re-hosting
  of the app — explicitly out of scope. (If that ever changes, the shared
  emitter generalizes from per-session to per-(session, size) — the
  single-encode is a side benefit, not a constraint to defend.)

## 3. Claiming — improvement ideas (the parking lot)

Scoped already in creature-comforts §2.5 (nearest-term tier):
- **~5s resize hysteresis** — challenger's keystrokes pass through, the
  *resize* waits until the current owner pauses. Kills resize ping-pong.
- **Ownership chip** in the pane header ("⌗ sized for rosson"), click-to-claim,
  quiet toast to the displaced owner.
- **"<user> is typing" hint** — the social signal that prevents wheel-grabs.
- **Auto-fit for non-owners** — per-terminal font auto-shrink to fit the
  owner's grid (CMD+Shift+± mechanism automated), 9px floor then clip+scroll,
  letterbox when larger; manual override; whole-app zoom untouched.
- Daemon bookkeeping: `last_interaction_at` per subscriber, username on the
  claim, `size_claim_changed` broadcast, claim cleared on owner disconnect.

Further out (unscoped — revisit when multi-user usage is real):
- **Claim lock / "presenting" mode** — the owner pins the size so typing by
  others doesn't steal it (demos, pairing-as-navigator). Needs an explicit
  unlock affordance + maybe owner/admin override.
- **Polite mode (request control)** — request/approve handoff instead of
  click-to-claim. Hosted-tier flavor; a trusted two-person team doesn't want
  the ceremony (Decision D7 in the review doc).
- **Role-weighted claims** — does an Owner outrank a Member's claim? Today:
  no, last-typist-wins is flat. Revisit with teams >2.
- **"Driving" indicator distinct from size ownership** — who typed last
  (driving) vs whose size the PTY wears (owner) can differ under hysteresis;
  maybe surface both, maybe deliberately don't (confusing).
- **Claim history in the activity feed** — "baden took control of claude
  (nsi-plan01) 14:02" for after-the-fact "who resized this" forensics.
- **Idle claim release** — owner disconnect already clears (scoped); should
  *idle* (no input for N min) also release so focus claims from others win
  without typing? Lean no — next typist claims anyway; revisit if the chip
  showing a long-idle owner confuses people.
- **Follow-the-claimant** — auto-jump your view to whatever session the
  current claimant is driving (composes with follow-the-activity from §3 of
  the creature-comforts PRD).
- **Mid-menu protection** — suppress *other* users' raw input while the owner
  is inside an interactive TUI menu? Probably unknowable (we can't reliably
  detect "in a menu"); the typing hint is the realistic mitigation. Captured
  so we remember we considered and rejected it.

## 4. Revisit triggers

Pull this doc back out when any of these happen:
- A real team (3+ users) runs a shared K2SO host day-to-day.
- The composer ships and we observe how much raw-PTY contention remains.
- Hosted tier (k2.dev) makes "polite mode" / role-weighted claims a sales ask.
- Anyone asks for "presenting" during a demo.
