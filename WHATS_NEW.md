# K2SO — What's New

User-facing highlights of recent updates. See `release-notes-X.Y.Z.md`
files in the repo root for the full developer-facing changelog.

## 0.39.24 — K2 Connect: open a workspace on the remote machine

- **Open folders that live on the host.** When you're connected to another
  machine, "New Workspace" now lets you browse and pick a folder on **that
  machine** — an in-app folder browser that walks the remote's filesystem —
  instead of your local file picker (which could only see this computer).
- **Friendlier with out-of-date machines.** The app stays compatible with
  hosts running an older K2SO, so you can always connect and sign in to
  update one. And when a host is too old for a newer feature, the app now
  tells you which version it needs instead of silently doing nothing.

## 0.39.23 — K2 Connect: roles + cleaner remote settings

- **User roles for shared servers.** Connect users now have a role:
  **Owner**, **Admin**, or **Member**. The owner can promote trusted people
  to help run the server (including handing off ownership); admins can add
  users and enable/disable them; members just connect and use it. Removing
  users and changing roles stay owner-only.
- **Cleaner settings when viewing another machine.** The K2 Connect
  *tunneling* controls — k2.dev sign-in, subdomain, start/stop — now hide
  while you're connected to a remote host, since those belong to the machine
  that owns the daemon. Managing **that** server's users still works from
  right there.
- **`k2so` works from any folder.** Fixed a bug where running the `k2so`
  command (for example, an agent-to-agent message) from a directory that
  isn't a git repository would exit silently with no output.

## 0.39.22 — Onboarding fixes + remote settings clarity

- **Agents spawn out of the box.** The background daemon can now find
  `claude`/`cursor`/`gemini` even when they're installed in `~/.local/bin`
  (the native Claude installer), Homebrew, nvm, etc. — previously it only saw
  a bare system PATH and failed with "command not found".
- **No more stuck-on-Connecting after an update.** If K2SO was ever launched
  straight from the mounted disk image, the daemon could get pinned to that
  stale copy and never pair after upgrading. It now self-heals its path on the
  next launch, and warns if you run it from the DMG instead of /Applications.
- **Settings shows which server you're on.** While connected to another
  machine, the top of Settings now displays (and lets you switch) the active
  server.

## 0.39.21 — K2 Connect: the client fully mirrors the host

When you connect to another machine, the **whole** app now reflects that
host — workspaces, the active bar, pinned/active lists, whether focus
groups are on, panels, custom themes, timer entries, and settings — instead
of bleeding through your local machine's state. Your own client preferences
(terminal look, file-tree options, window layout) correctly stay yours.

## 0.39.20 — K2 Connect: remote clients can read the host's data

Fixes the bug where connecting to another machine showed *your* workspaces
instead of the host's. The host daemon was refusing a connected user's
session on every data read (workspaces, files, git), so the client silently
fell back to showing local data. Now a connected client sees the host's
workspaces, files, and git as intended. Update the **host** machine to 0.39.20.

## 0.39.19 — K2 Connect: driving a remote machine, done right

Connecting to another machine now makes the **whole** app follow it —
workspaces, files, git, agents, settings — reliably. This reworks the
0.39.18 approach from the inside: the app talks to the connected daemon
directly instead of proxying through this machine. That removes the
freeze-on-connect and fixes the bug where a failed connection could blank
your local workspace list until a reload.

## 0.39.18 — K2 Connect: actually drive the remote machine

When you connect to another machine, K2 now shows **that machine's**
workspaces, files, git, and agents — not your local ones. Previously the
connection succeeded but most panels kept showing this computer; now the
whole app follows the daemon you're connected to.

Also fixes the "Invalid or missing auth token" error on connect (the app
now waits for your sign-in before loading), and **bundles the tunnel client
(`frpc`) inside K2** — a fresh host machine can start a secure tunnel with
no manual install.

## 0.39.17 — K2 Connect sign-in fix

Fixes a "Load Failed" error when signing in to your k2.dev account (and
when connecting to a remote machine) in the packaged app. The production
build was blocking the secure connections K2 Connect needs; signing in,
loading your subdomains, and connecting to another machine over
`https://<you>.k2.dev` all work now.

## 0.39.16 — K2 Connect: reach your workspace from anywhere

K2SO can now expose your daemon at your own **`https://<you>.k2.dev`**
address, so you can reach this machine from another computer.

Sign in to your k2.dev account right in **Settings → K2 Connect**, pick a
subdomain you own, and hit **Start** — your machine goes live over a secure
tunnel. It can re-launch the tunnel automatically when the daemon restarts,
and if the same subdomain is already running on another device it's greyed
out so the two don't clash (swapping asks first).

Decide **who** can connect in: under **Users / Access** add people with a
username + an initial password (you set it once and can reset it, but never
see it again), choose your password rules (length, special characters), and
they manage their own password in a browser at your `k2.dev` address. To
connect *to* another machine, add it under **Connections** with its URL,
username, and password.

Settings → K2 Connect and Connections are now a single page, and the
K2 Companion page points you to the mobile app.

## 0.39.15 — No more phantom "audit bucket" projects in the sidebar

New users no longer see two confusing entries — **"Orphan audit bucket"**
and **"Broadcast audit bucket"** — in the workspace sidebar. Those are
internal bookkeeping items (they route the activity feed behind the
scenes); they were never meant to look like workspaces you created. They
now stay hidden from the project list while still doing their job
internally.

## 0.39.14 — Pinned Chat/Inbox tabs always point at the right workspace

Fixes a bug where a workspace's **pinned Chat and Inbox tabs** could stay
stuck pointing at a *different* workspace — wrong agent, wrong folder,
and (for Chat) the wrong conversation. New terminal tabs always opened
in the right place, but the pinned tabs kept routing to the other
workspace, and there was no way to fix it from the app.

It happened mainly to workspaces **created from inside another
workspace** (e.g. spinning up a new workspace from within an existing
one's chat) — the new workspace's pinned tabs picked up the parent's
context and held onto it.

Now K2SO **re-checks and corrects** a pinned tab's workspace every time
you switch into it, so any affected workspace **heals itself the next
time you open it** — no reinstall, no settings to touch. (When the
workspace is corrected, the pinned Chat tab also starts a fresh
conversation, since the old one belonged to the other workspace.)

A follow-on to 0.39.12's terminal-stall fix that removes the root cause
rather than just the symptom.

K2SO keeps every workspace's terminal session running in the background
(that work never pauses) — but until now the app also kept a **live
data stream open for every one of them**, even sessions you weren't
looking at. With many workspaces and a long-running app, that piled up
into a lot of redundant streaming, which was the underlying driver of
the terminal stalls.

Now the app **streams only the terminal pane that's actually on
screen.** When you switch tabs or workspaces, it stops streaming the
one you left and starts streaming the one you land on — instantly, with
no loss, because the session itself keeps running in the daemon the
whole time. Background sessions stay fully alive and keep working; the
app just doesn't waste resources rendering them when you can't see them.

The result: dramatically less background load, and terminals stay
responsive no matter how many workspaces you have open or how long
the app has been running.

Two fixes from user reports.

**Terminals no longer stall and "catch up."** If you run with more than
one terminal open — or just keep K2SO running for a while with several
workspaces — terminals could freeze for a few seconds and then suddenly
jump back to life, getting worse the longer the app was open. The cause
was an internal "who's the live view of this terminal?" signal that
every open pane was claiming at once, including hidden background tabs,
whenever the window had focus. With many sessions that turned into a
constant tug-of-war that flooded the terminal's live-update channel —
the flood is what you saw as the freeze, and the recovery is what you
saw as the sudden catch-up. Now only the **one terminal you're actually
looking at and typing in** claims that role, so the tug-of-war can't
happen no matter how many sessions are open or reconnecting.

**Chat history shows the workspace you're in.** Opening the chat-history
panel inside one workspace could show *another* workspace's chats — the
one that happened to be globally active (usually whichever has agents
running). The panel now binds to the workspace it's opened from, so you
always see that workspace's own history.

## 0.39.11 — Self-healing window: no more black screen after sleep or update

If K2SO ever opened to a **black, unresponsive window** — after an
update, or after your laptop slept and woke — this release makes it
recover on its own.

The root cause was the app's renderer occasionally not coming back to
life: the window's web layer would load but never start running, most
often right after an auto-update or when the Mac's app-rendering
process gets killed during sleep/wake. Until now the only fix was the
hidden right-click → Reload, which ordinary users would never think to
do — so the app just looked broken.

K2SO now watches its own window with a lightweight heartbeat. If the
interface stops responding, the app **automatically reloads it from
the native side** (the same thing the manual reload did) and brings
it back within a few seconds — no clicking required. It covers both
the after-update case and the after-sleep case, and it won't touch a
window that's working fine.

Also: the update button in Settings → General now reads **"Download"**
instead of "Download & Install" (the install happens when you click
the separate "Install & Relaunch" button).

## 0.39.10 — Read another agent's terminal + agent-setup fix

Three improvements for working with agents.

**`k2so read <workspace>` — look over another agent's shoulder.** The
read complement to the messaging verbs: `msg` talks live, `inbox` is
mail, and now `read` shows you the last N lines of another workspace's
live terminal. Great for human-in-the-loop — peek at what an agent is
doing or waiting on *before* you send it a message, or diagnose one
that's gone quiet:

```
k2so read <workspace>                 # last 50 lines of its session
k2so read <workspace> --lines 120     # more history
k2so read <workspace> --agent <name>  # a specific agent's session
```

**`msg` length limit is now documented.** Live `msg` is for short,
single-line messages — it's injected into the recipient's input line,
so long or multi-line text gets truncated. For anything substantial
(task briefs, file contents, multi-line notes) use the inbox, which has
no length limit: `k2so msg <workspace> --inbox --title "..." --body "..."`.
That length limit is the whole reason the inbox exists.

**Fixed: new agents are set up in the right place.** When you turned a
workspace into a Custom or K2SO agent, its persona file could get
scaffolded into a legacy `.k2so/agents/` folder instead of the canonical
`.k2so/agent/AGENT.md` — so an agent's documentation could land
somewhere the rest of K2SO wasn't looking. New agents now go to the
correct location, and any workspace already affected gets its agent
files moved back automatically on the next launch (your content is
preserved).

## 0.39.9 — Hotfix: exited terminals stay exited

Hotfix for a regression introduced in 0.39.8's reconnect logic. If a
terminal's child process exited (you closed a shell with `exit`, an
agent ran to completion, etc.) at exactly the moment the daemon's
WebSocket connection dropped, the new reconnect path would
incorrectly resurrect the exited terminal as a brand-new shell
session — visually confusing and wrong.

0.39.9 fixes that: an exited terminal stays exited, even if the
WebSocket teardown races the child-exit event. Normal mid-flight
WebSocket reconnects (the actual fix from 0.39.8) work exactly as
before.

If you didn't see any weird "my terminal that ran to completion came
back as a fresh shell" behavior on 0.39.8 — congrats, you weren't
hitting the race; just update and move on.

## 0.39.8 — Terminal panes recover from network blips + no more "frame stalls"

Two distinct long-running-session bugs fixed, both reported with
deep diagnostic profiles by external users. Combined with 0.39.7's
fd-exhaustion fix, multi-hour K2SO sessions should now stay smooth.

### Terminal panes survive network blips (was: silently frozen until quit)

Before: if a WebSocket between K2SO and its background daemon dropped
mid-flight — TCP reset, macOS App Nap, network blip — the terminal
pane went silently dead. Last frame stayed on screen, keystrokes
went nowhere, and the only fix was to quit and relaunch the app.
Cause: the WS `close` handler was a no-op, with no reconnect path.

Now: each terminal pane automatically reconnects within ~500 ms of
a drop (with a brief backoff for sustained outages). The PTY
session survives intact — your shell history, scrollback, and
running program continue. You'll see the pane go to "Connecting…"
briefly and then come back to life.

The session-events subscription (which keeps the workspace sidebar
in sync) got the same treatment: any error or close now triggers
an idempotent reconnect. Closes a gap where WebKit Networking
hiccups could leave that channel silently dead.

### Terminal output no longer "freezes then snaps"

Before: in long sessions, every terminal could intermittently freeze
for a beat then "catch up" all at once. The renderer was hot-looping
focus claims/releases to the daemon, eventually overrunning the
daemon's broadcast buffer by **thousands** of frames; recovery
required the daemon to flush a fresh full-grid snapshot. During the
overrun window all subscribers stopped seeing live updates.

Now: focus claims are deduplicated at the WebSocket-send level, so
the daemon only hears about real focus transitions (not React
re-render noise). In the common single-viewer case, the channel goes
completely silent except for legitimate user-driven focus changes
— and the broadcast buffer never overruns.

Thanks to the users who profiled both of these in production
(Issues #3 and #5) and submitted complete fix recommendations along
with their diagnoses.

## 0.39.7 — No more "K2SO slows down over the hour" lockups

Bug-fix release. If you ever ran K2SO for ~45 min to an hour and watched
it progressively slow down — file tree's `loading…` indicator lengthening,
terminals stalling for stretches, then everything "coming back to life
out of nowhere" — this is the release that ends it.

**What was happening:** every fetch the app made to its own background
daemon was a brand-new TCP connection, because the daemon forced
`Connection: close` on every response. macOS's web-renderer process
has a default cap of 256 sockets, and it cleans them up slowly. Over
~50 minutes of normal use the leftover sockets piled up against that
ceiling, and new requests had to wait for the kernel to time out old
sockets before they could go through. That's the "loading…" lengthening
and the freezes-then-recovery you saw.

**What's fixed:** the daemon now reuses one TCP connection for many
requests (standard HTTP keep-alive). Sockets recycle properly; the
~50-min wall is gone. A user reported the bug with a full live-CPU +
`lsof` + `sample` profile that nailed the root cause — credit to them
for the diagnosis.

Nothing for you to do — just smoother long sessions from here on.

## 0.39.6 — Terminal-stall storm fixed

Bug-fix release. If you ever saw every terminal session **lag or stall
for about 15 seconds at once** — usually with a lot of agent terminals
open — then "come back to life" on its own, this is the release that
ends it.

**What was happening:** the renderer's "Active agents" sidebar polled
every running terminal individually every 2.5 seconds, firing one
small HTTP request per terminal. On a box with many agent terminals
that meant a periodic flood of requests through the WebView's
networking stack — enough to spike renderer CPU to 80–128% and stall
every terminal at once until the storm cleared. The daemon was idle
throughout (a victim, not the cause).

**What's fixed:** the sidebar now makes **one request per poll**
instead of one per terminal, and stops re-rendering the active-agents
list when nothing has actually changed. Behaviour is identical — same
agents detected, same idle/active dots — just without the
request-storm side-effect.

Thanks to the user who profiled this in production and submitted the
fix.

## 0.39.5 — No more blank window after an update

Bug-fix release. If you ever updated K2SO and landed on a blank/black
window that only a right-click → Reload could fix, this is the release
that ends it — especially when updating from an older version that has
a lot of one-time setup to do on first launch.

**What was happening:** during an update, the app could briefly talk to
the *old* daemon that was on its way out, mount against it, and then get
stranded when the new daemon was still busy applying updates. No crash —
just a window that never finished loading.

**What's fixed:** the app now refuses to start against anything but the
daemon that ships with this exact build, and the daemon reports its
progress while it works. So instead of a blank window you'll see a
brief **"Setting up K2SO — applying updates…"** while first-boot
migrations run, then the app opens normally. On a big upgrade that
setup can take a few seconds; you'll see it happening rather than
staring at black.

Nothing for you to do — just smoother updates from here on, no matter
how old the version you're coming from.

## 0.39.4 — What's New popup: walk back to 0.39.0

Tiny UX fix to the "What's new" popup itself. Before: if you landed
mid-track on a 0.39.x patch (e.g. you updated 0.39.2 → 0.39.3 via
auto-update), the popup only showed entries newer than the version
you'd last dismissed — so you missed the foundational **0.39.0**
entry that explains why your workspaces sidebar got rearranged.

Now: while you're anywhere on the 0.39.x minor track, the popup
always carries **every 0.39.x entry** up through the version you
just installed.

**👈 Hit the ← arrow at the bottom-left of this popup to walk
back through 0.39.3, 0.39.2, 0.39.1, and 0.39.0.** The 0.39.0
entry is the one to read if you're wondering where your "Agents"
section went or why some workspaces are now pinned — it's the
release where that all changed, and it's only one ← away.

The same behaviour will hold for every future minor: land
anywhere on the X.Y.* track, walk back to read the whole story.

## 0.39.3 — ConnectionGate fix: no more black screen after update

Patch release. 0.39.2's ConnectionGate gated the *render* of the
app but still loaded the entire app's modules at startup. Several
stores fire daemon fetches the moment they're imported — if the
daemon was still kickstarting (the auto-update scenario), those
fetches failed and the stores got stuck in a broken state, leaving
the app rendering as a black window even after the gate dismissed.

0.39.3 defers loading the app's modules until the daemon is verified
healthy. App imports happen for the first time AFTER the gate sees a
green daemon — so every store's initial fetch hits a daemon that's
ready to respond. The black-screen-then-reload workaround is gone.

Bonus polish: the Reload button on the Connecting screen now appears
after 10 seconds (was 30), with friendlier copy explaining that the
daemon may still be loading and offering both "quit + relaunch" and
"reload" as recovery options.

## 0.39.2 — ConnectionGate: render after daemon healthy

Patch release. Fixes the "blank screen after update" race that some
users saw when 0.39.1 landed via auto-updater. A new ConnectionGate
component shows a small "Connecting…" overlay while it waits for the
daemon to be reachable, then mounts the app once it responds. No
more "right-click → Reload to make it work" on first launch after
update.

Bonus: this is the same primitive K2 Connect will use when
connecting to remote daemons (where transient unreachability is
normal). So the architecture lands now and pays dividends later.

## 0.39.1 — Manager-pin fix

Patch release. 0.39.0's one-shot migration over-pinned workspaces in
**manager mode** (manager / coordinator / pod) — the pre-0.39.0
sidebar only auto-promoted **K2SO Agent** and **Custom Agent**
workspaces, so manager-mode shouldn't have been pinned by the
migration. 0.39.1 ships a corrective one-shot migration that unpins
those manager-family workspaces on first launch.

**This only happens once.** After the corrective migration runs,
your pin choices are yours to keep — re-pin any manager workspaces
you want at the top (right-click → Pin) and they stay pinned across
all future versions.

## 0.39.0 — Clean foundation: new CLI, unified sidebar, chat/inbox everywhere

The first public release after a major behind-the-scenes refactor. K2SO
got a lot tidier — same product, cleaner bones. Things you'll notice:

- **Workspaces sidebar simplified.** The "Agents & Pinned" auto-promote
  behavior is gone — agent-mode workspaces no longer get a dedicated
  section forced above your manually pinned workspaces. **A one-time
  migration on first launch pins every workspace that was in agent
  mode** so nothing moves on you visibly: the workspaces that lived in
  the auto-promoted Agents section will still appear at the top of
  your Pinned list. If you don't want them pinned, right-click → Unpin
  any of them — they'll flow into the normal ungrouped / focus-group
  sections. Future workspaces you switch into agent mode won't
  auto-pin; you decide where they go. Same for the Workspaces Settings
  page where you organize what shows up in your nav.

- **Chat + Inbox tabs visible for every workspace** — even ones with
  agent mode set to "off". Every workspace is reachable via cross-
  workspace messaging (`k2so msg <workspace>`), so the inbox surface
  is always available now. Previously these tabs hid when agent mode
  was off, which made the receive side invisible.

- **New CLI** with 24 cleaner verbs across daily / power / internal
  tiers. Old verbs like `k2so delegate`, `k2so work create`, `k2so
  who`, `k2so roster` now print a helpful error pointing at their
  replacement (`k2so inbox compose`, `k2so connections list`, etc.).
  See `release-notes-0.39.0.md` for the full deprecation map.

- **Storage shapes consolidated**: `.k2so/work/` → `.k2so/inbox/` and
  `.k2so/agents/<name>/` → `.k2so/skills/<harness>/`. The daemon
  migrates existing workspaces on first launch; no manual steps
  required. Your inbox items and skills survive — they just live in
  cleaner paths now.

- **Daemon-first foundation.** Most logic moved from the desktop shell
  into the daemon so the same code can power K2 Connect / K2 Companion
  (coming in 0.40.0). Mobile companion's pending-reviews badge, the
  desktop Review Queue UI, the heartbeat triage, and `cli` commands
  all share one source of truth now. **Bug fixes shipping with this**:
  `/cli/agentic` settings no longer 400s, review queue no longer
  silently shows 0, regenerated SKILL.md / CLAUDE.md / AGENT.md use
  the new CLI verbs, chat-history dedup for Pi / Codex / Cursor-IDE
  parsers, trash test infra hardened against macOS Touch ID flakes.

Plus a long list of internal cleanup — see `release-notes-0.39.0.md`
for the developer-facing catalog.

## 0.38.13 — Faster launch + smarter memory threshold

Cleanup pass on 0.38.12's two big additions:

- **Launch speed.** The What's New popup's "is daemon ready?" retry
  loop used to block a Tauri worker thread for up to 5 seconds at
  app startup, contending with all the other launch-time work. Now
  the retry happens renderer-side via plain `setTimeout` (yields
  between attempts) and the popup's first check is deferred until
  2 seconds after the rest of the UI has painted.
- **Smarter memory warning.** The 800 MB threshold was firing
  immediately on app launch because the local LLM loads ~1+ GB of
  weights into the process address space. The watcher now captures
  a settled baseline at the second sample and warns only on
  **growth** above that (+800 MB) or a hard ceiling (3 GB). Either
  signals a real leak; LLM steady-state is silent.

## 0.38.12 — Memory watcher + quieter heartbeat audit log

Two improvements driven by an overnight crash report:

- **Renderer memory watcher.** K2SO now logs its own memory usage
  every 5 minutes (visible in the Web Inspector console as
  `[k2so/memory] rss=...MB`). If the app ever crosses 800 MB you'll
  see a toast suggesting a restart. Gives us telemetry to catch
  Tauri-side memory leaks before macOS reaps the app under pressure.
- **Heartbeats auto-disable when WAKEUP.md is missing.** Before:
  a deleted or unreadable WAKEUP.md caused the heartbeat to retry
  every tick, spamming the audit log with `failed to compose wake
  prompt`. Now: the heartbeat flips to disabled on the first miss,
  records a single `auto_disabled` audit entry, and stays quiet
  until you fix the file and re-enable it from Settings →
  Heartbeats.

## 0.38.11 — Split popup into auto-fire vs button-trigger

Small architecture fix to the "What's new" popup. The popup is now
explicitly two-purpose:

- **Auto popup on the main screen** — fires once on first launch
  after a K2SO update. Same as before.
- **Button popup in Settings** — only opens when you click
  **Read what's new** in Settings → General. Never auto-fires when
  you happen to open Settings between updates.

Same modal UI in both places; just cleaner separation under the hood.

## 0.38.10 — Heartbeats on freshly-flipped agent workspaces

Hotfix: if you flipped a workspace to Custom / Workspace Manager /
K2SO Agent and immediately tried to add a heartbeat, you'd hit
"No scheduleable agent found in this workspace." Cause: the
validation was looking at `.k2so/agent/AGENT.md` on disk to confirm
"this is an agent workspace," but a mode-flip writes the DB
declaration immediately while AGENT.md may not be written yet.

Now: heartbeat add/remove/rename trust `projects.agent_mode` — the
column that's the source of truth for "this workspace is configured
as an agent." If the mode is set, you can schedule heartbeats
without waiting for any specific file to appear on disk.

## 0.38.9 — "Read what's new" works while Settings is open

Tiny hotfix to 0.38.8's new Settings button. Before: clicking
**Read what's new** in Settings → General appeared to do nothing —
the popup only appeared after you closed Settings. Cause: the popup
component wasn't mounted in the Settings-open layout, so the
button's open-popup event had nowhere to land.

Now: the popup opens immediately on top of Settings, no need to
close anything first.

## 0.38.8 — Cmd+T tabs remember their conversations + popup fixes

Two follow-ups to 0.38.5 and 0.38.7:

- **Cmd+T `claude` tabs now resume their conversations** across daemon
  restarts (app updates, kickstart, crash). Before: tabs came back as
  fresh claude sessions. Now: they pick up exactly where you left off,
  same as pinned chat does.
- **"What's new" popup wasn't appearing** for some users after the
  0.38.7 update — the renderer was checking the daemon before
  credentials were written, missing the popup entirely. Fixed with a
  short retry window so it survives launch races.
- **New "Read what's new" button** in Settings → General (under the
  CLI version row) — reopen the popup anytime to re-read what changed
  in the current release.

## 0.38.7 — Update notes when K2SO updates

You're seeing this because K2SO now shows a small "what's new" popup
the first time you open the app after an update. It rolls up everything
you missed if you skipped a version or two — no more wondering what
changed.

- Friendly per-update highlights
- Catches you up across multiple versions if you skipped a few
- `k2so whatsnew` reprints them anytime from the terminal
- `k2so whatsnew --reset` makes the popup show again next launch
  (good for sharing with a teammate)

## 0.38.6 — Inter-agent messages just work

`k2so msg <workspace> "text"` now delivers reliably on the first try.
The "send it twice and pray" workaround that agents were using is no
longer needed.

- One canonical JSON response shape every call — no more guessing
  whether `injected_to_pty: true` actually meant delivered.
- When delivery fails, you get a specific `reason` and an actionable
  `hint` instead of a silent inbox fallback.
- Recipients see `[from <sender>]` prefixed on every message, so they
  always know who's talking.
- `--wake` is no longer needed — `msg` is always live. Use
  `k2so work send` when you actually want to queue something for later.
- `k2so msg --help` finally works.

## 0.38.5 — Cmd+T tabs survive app updates

Your terminal tabs (including pinned chat) keep their `claude` sessions
through app updates and daemon restarts.

Before: a tab opened with `claude` would become a plain shell after the
next K2SO update. Now it comes back as `claude` — same command, same
working directory, same args. Subsequent updates won't reset your tabs
back to a shell.

## 0.38.4 — Heartbeats panel polish

The Heartbeats settings panel now matches the rest of the app's theme.
Heartbeat list is sorted alphabetically (case-insensitive — workspaces
named `alakazam-labs-website` and `BIG-CRM` no longer cluster apart).
Cosmetic only; no behavior change.

## 0.38.3 — System-wide Heartbeats settings page

Added a right-hand panel to the Heartbeats settings showing every
heartbeat across every workspace with toggles for enable/disable,
pinned-chat opt-in, and edit-wakeup. Plus a third column for a running
audit log of every fire system-wide — so you can finally see at a
glance which heartbeats are firing and which are dark.

## 0.38.2 — Heartbeats finally fire reliably

If you had heartbeats configured but they hadn't been firing for a
while (sometimes weeks), 0.38.2 fixes it. We replaced our hand-rolled
scheduler with the well-tested `croner` crate. Heartbeats now recover
cleanly from any pause and fire on schedule.

## 0.38.0 — Daemon-authoritative tabs + multi-window sync

Terminal tabs, including the pinned chat, now persist correctly when
the Tauri app closes and reopens — the daemon owns the sessions, and
the renderer attaches to whatever's already running. Cross-window
state (heartbeats minimized, pinned chat refresh, etc.) syncs
automatically.
