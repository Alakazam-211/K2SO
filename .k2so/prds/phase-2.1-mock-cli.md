# Phase 2.1 Mock CLI — what the redesigned `k2so` will look like

**Purpose**: a captured-output mock of the post-Phase-2.1 CLI for fresh-eyes UX validation. This is what an agent would see if they ran each help command. **Nothing here is implemented yet** — it's the spec rendered as output.

**Revised 2026-05-23** with the inbox-as-email model (A22), `delegate` deprecation (A23), filesystem rename (A24), and glossary additions per the fresh-eyes review's findings.

---

## `k2so help` (default — daily tier, 14 verbs)

```
K2SO CLI — workspace orchestration for AI agents

A workspace = a project folder K2SO knows about. Each workspace has
one primary agent that reads its inbox, applies skills to work, and
coordinates with other workspaces.

TALK TO OTHERS
  msg <workspace> "text"                Live delivery (call/IM — blocks until landed)
  msg <workspace> "text" --inbox        Drop in their inbox (email — async)
  msg <workspace> --signal <kind> ...   Emit a typed signal (advanced)
  who                                   Workspaces with live agents right now
  connections list|add|remove           Cross-workspace links

YOUR INBOX
  inbox                                 Show inbox items (default)
  inbox list [<folder>]                 List items in inbox or a folder
  inbox read <id>                       Full text of one item
  inbox compose --title "..." --body    Write your own item (self-note / task)
  inbox respond <id> "text"             Reply (back to sender)
  inbox move <id> <folder>              File into folder (creates if needed)
  inbox archive <id>                    Standard archive (preserved + searchable)
  inbox delete <id>                     Move to macOS Recycle Bin (recoverable from Trash)
  inbox search "query"                  Search inbox + folders
  inbox folders                         List folders this workspace has created

CHECKIN
  checkin                               Heartbeat ping ("I'm alive")
  checkin --status "message"            Report status update
  checkin --done [--blocked "reason"]   Task complete (or blocked)
  done                                  Shortcut for `checkin --done`

WORKSPACE
  workspace list                        Yellow pages: every workspace + status
  workspace list --running              Filter to live agents
  workspace launch [<path>]             Smart cascade: attach|wake|spawn
  workspace profile [<path>]            Read workspace agent's AGENT.md
  workspace update --field <f> --value <v>
                                        Edit workspace agent profile

REVIEW IN-FLIGHT WORK
  reviews                               Pending merge reviews for worktrees
  review approve|reject|feedback <agent> [options]
                                        Act on a pending review

ACTIVITY
  activity [--limit N] [--workspace <p>]
                                        Audit log of workspace events
  commit [-m "..."] [--merge]           AI-assisted commit (--merge to also merge)

INFO
  help                                  This help
  help --advanced                       Power-user verbs (heartbeat, daemon, etc.)
  help --internal                       Orchestrator-RPC verbs (rare)
  help-deprecated                       Map: old verb → new equivalent
  glossary [<term>]                     Define K2SO-specific terms (try `glossary inbox`)
  whats-new                             Changelog popup
  version                               CLI version

ENVIRONMENT
  K2SO_DAEMON_URL     Remote daemon URL (default: localhost — see K2SO Connect)
  K2SO_PROJECT_PATH   Workspace root (default: $PWD)
  K2SO_AGENT_NAME     Default sender for `msg --from`

Run `k2so glossary <term>` for any unfamiliar word (workspace, skill,
inbox, heartbeat, etc.).
```

---

## `k2so help --advanced` (power-user, 6 verbs)

```
K2SO CLI — Power-user surface

Includes all daily verbs above, plus:

HEARTBEATS (workspace-scoped scheduled wakes)
  heartbeat schedule add --name <n> <spec>      Create heartbeat schedule
  heartbeat schedule list [--archived]          Active or archived
  heartbeat schedule remove <n> [--purge]       Soft-archive (default) or hard-delete
  heartbeat schedule unarchive <n>              Restore from archive
  heartbeat schedule edit <n> <spec>            Change schedule
  heartbeat schedule rename <old> <new>         Rename folder + DB row
  heartbeat schedule enable <n>                 Resume scheduling
  heartbeat schedule disable <n>                Pause scheduling

  heartbeat signal fire <n>                     Fire one heartbeat now
  heartbeat signal wakeup <n>                   Open the WAKEUP.md
  heartbeat signal wake                         Auto-wake (no name needed)

  heartbeat show <n> [--json]                   Single heartbeat details
  heartbeat status <n> [-n N]                   Recent fire history
  heartbeat log [-n N]                          Scheduler decisions

DAEMON LIFECYCLE
  daemon status [--json]                        PID, port, uptime
  daemon start|stop|restart                     launchctl bootstrap/bootout
  daemon log [--lines N]                        Tail daemon log
  daemon companion start|stop|status            Companion (ngrok tunnel) server
  daemon hooks status [--limit N] [--json]      Hook pipeline state
  daemon reap                                   GC dead-PID sessions
  daemon uninstall                              Remove daemon plist

SETTINGS
  settings                                      Show current settings
  settings --mode <off|agent|manager>           Workspace mode
  settings --state <build|managed|maintenance|locked>
                                                Workspace capability tier
  settings --agentic <on|off>                   Global agentic systems toggle
  settings --companion <on|off>                 Enable/disable companion tunnel
  settings --companion-password "<pw>"          Rotate companion password

SKILLS (documentation profiles — NOT spawnable; harness handles spawn)
  skills list                                   Skill profiles in this workspace
  skills create <name> [--template <role>]      Create a new skill profile
  skills remove <name>                          Delete skill profile
  skills profile <name>                         Read skill's SKILL.md/AGENT.md
  skills regenerate [<name>]                    Refresh SKILL.md files

UPDATE
  update                                        Update K2SO app (default: --app)
  update --app                                  Same: check/install app updates
  update --cli                                  Update this CLI script
  update --list                                 Show available versions

ONBOARDING (first-launch flow)
  onboarding scan                               Find adoptable projects
  onboarding adopt <path>                       Register an existing folder
  onboarding defer <path>                       Skip onboarding for now
  onboarding start-fresh <path>                 Create new workspace from scratch
```

---

## `k2so help --internal` (orchestrator-RPC, 4 verbs)

```
K2SO CLI — Internal surface (rare; used by agent runtimes)

These verbs exist for orchestrators and tools that integrate with K2SO.
Humans rarely run them directly.

TERMINAL (raw PTY I/O)
  terminal spawn --command "..."                Spawn a sub-terminal
  terminal write <id> "text"                    Paste + Enter to PTY by id
  terminal read <id> [--lines N]                Last N lines from buffer

SESSIONS (low-level session lifecycle)
  sessions spawn --agent <n>                    Session Stream spawn
  sessions list [--json]                        Raw session map
  sessions live                                 Subscribe to session events
  sessions compact                              Compact archive ring

AGENT (single-item ops on the workspace's primary agent)
  agent profile [<path>]                        Equivalent to `workspace profile`
  agent update --field <f> --value <v>          Equivalent to `workspace update`
  agent complete --file <f>                     Mark work item complete

HOOKS (Claude Code / Cursor integration state)
  hooks status [--limit N] [--json]             Hook pipeline state
                                                (same as `daemon hooks status`)

For "what's running" use `workspace list --running` (daily tier).
For workspace-scoped ops use the `workspace` verb (daily tier).
```

---

## `k2so glossary` (no arg: list all terms)

```
K2SO glossary — definitions of K2SO-specific terms

  activity         Append-only audit log of every workspace event
  agent            The workspace's primary AI assistant (1:1 with workspace)
  agentic          Global toggle for K2SO's autonomous systems
  companion        Daemon's ngrok-tunneled server (Mobile + K2SO Connect)
  connections      Cross-workspace links: which workspaces can read each other's status
  harness          IDE integration layer (Claude/Cursor config K2SO writes)
  heartbeat        Workspace-scoped scheduled wake (cron-like)
  hooks            Claude Code / Cursor integration hooks (NOT git hooks)
  inbox            Workspace's email-like communication channel
  onboarding       First-launch flow for registering a workspace
  signal           Typed event sent between workspaces (msg, status, presence, etc.)
  skill            Documentation profile for a role/capability
  skill-template   Master skill definition that can be instantiated
  state            Workspace capability tier (build/managed/maintenance/locked)
  workspace        A project folder K2SO knows about (has exactly one primary agent)
  worktree         Git worktree — a working directory linked to a different branch

Run `k2so glossary <term>` for a full definition.
```

---

## `k2so glossary connections` (single-term lookup)

```
connections     Cross-workspace links. When workspace A "connects" to
                workspace B, A can read B's `who` / `activity` / inbox
                presence; B can show up in A's `workspace list`. Used
                for declaring "these workspaces work together" so K2SO
                surfaces the right context.

                Manage via: `k2so connections list` / `add <path>` /
                `remove <path>`. Connections are symmetric (both sides
                see each other) and persisted per-workspace.

                NOT the same as: skill profiles (`k2so skills`), live
                sessions (`k2so workspace list --running`), or ngrok
                tunnel state (`k2so daemon companion`).
```

---

## `k2so glossary inbox` (single-term lookup)

```
inbox           The workspace's email-like communication channel. Items
                arrive here from other workspaces (via `k2so msg --inbox`)
                or are composed by the workspace's own agent (via
                `k2so inbox compose`).

                Inbox items are non-urgent, non-aggro — the agent reads
                and triages on its own schedule. Triage = move items
                into folders the agent creates. There's no system-imposed
                folder taxonomy; the agent organizes its inbox the way
                a person organizes email (Projects, Reference, Issues,
                FYI, etc.).

                Storage: `.k2so/inbox/<id>.md` (top-level) and
                `.k2so/inbox/<folder>/<id>.md` (after `inbox move`).

                Migration from pre-Phase-2.1 K2SO: the daemon runs a
                one-shot migration on its first boot after upgrade.
                Old `.k2so/work/{inbox,active,done}/*.md` files are
                atomic-renamed into `.k2so/inbox/{,active,done}/`, then
                the empty `.k2so/work/` folder is sent to the macOS
                Recycle Bin (recoverable if anything was missed). After
                migration there's no `.k2so/work/`; everything lives
                under `.k2so/inbox/`.

                See also: `k2so inbox --help` for the full verb surface,
                `k2so msg --help` for sending into someone else's inbox.
```

---

## `k2so glossary skill` (single-term lookup)

```
skill           A documentation profile describing a role, persona,
                and instructions. Skills are *not* spawnable entities —
                they're markdown files (SKILL.md) that your harness
                (Claude Code, Cursor) loads when you want to apply
                that role to specific work.

                K2SO manages skill files (list, create, remove, profile,
                regenerate). Spawning a session pre-loaded with a skill
                is your harness's job (sub-agent spawning in Claude
                Code, etc.) — K2SO no longer provides a `delegate`
                verb for this.

                Filesystem: skills currently live at
                `.k2so/agents/<name>/` (historical naming from when
                they were called sub-agents). Templates at
                `.k2so/agent-templates/<role>/`. Rename to
                `.k2so/skills/` is planned for Phase 3.

                See also: `k2so skills --help`, `k2so glossary agent`.
```

---

## `k2so glossary agent` (single-term lookup)

```
agent           The workspace's primary AI assistant. K2SO enforces a
                1:1 invariant: agent and workspace are one entity —
                use `workspace` verbs to manage both.

                The agent reads the workspace's inbox, applies skills
                to work, fires on heartbeat schedules, and coordinates
                with other workspaces via `msg`. It's the "user" of
                the workspace from K2SO's perspective.

                See also: `k2so workspace profile` (reads the agent's
                AGENT.md), `k2so glossary skill` (capability profiles
                the agent can apply).
```

---

## `k2so glossary heartbeat` (single-term lookup)

```
heartbeat       A workspace-scoped scheduled wake (cron-like) that
                fires the workspace's agent at defined intervals.
                Used for: periodic triage, scheduled syncs, "wake me
                at 9am every weekday and check the inbox" patterns.

                A workspace can have multiple heartbeats with different
                names + schedules. Manage via `k2so heartbeat schedule
                add|list|remove|edit|enable|disable`. Fire one
                immediately via `k2so heartbeat signal fire <name>`.

                Storage: `.k2so/heartbeats/<name>/` per heartbeat.
                The daemon owns the launchd plist that fires them
                (`com.k2so.agent-heartbeat.<workspace>.plist`).
```

---

## `k2so glossary worktree` (single-term lookup)

```
worktree        A git worktree — a working directory linked to a
                separate branch from your main checkout. K2SO uses
                worktrees so a sub-agent or pull request can work on
                a feature branch in isolation, then merge back.

                Modern harnesses (Claude Code, Cursor) create worktrees
                natively when you spawn a sub-agent. K2SO tracks the
                resulting worktrees in `reviews` (pending merges) and
                the daemon owns cleanup on merge or reject.

                See also: `k2so reviews`, `k2so review approve|reject|feedback`.
```

---

## `k2so inbox --help`

```
k2so inbox [<subcommand>] [options]

The workspace's email-like communication channel. Items arrive from
other workspaces (via `k2so msg --inbox`) or are composed by your own
agent (via `k2so inbox compose`). Read, triage, file into folders.

SUBCOMMANDS
  (none)                    Show inbox items (= `inbox list`)
  list [<folder>]           List items in inbox (or a specific folder)
  read <id>                 Full text of one item
  compose --title "..." --body "..."
                            Write your own inbox item (self-note / task)
  respond <id> "text"       Reply (back to sender)
  move <id> <folder>        File into a folder (creates folder if needed)
  archive <id>              Standard archive (preserved + searchable; goes to inbox/done/)
  delete <id>               Move to macOS Recycle Bin (recoverable from Trash)
                            Use `inbox archive` for "done but keep around";
                            use `inbox delete` only for "actually remove this."
  search "query"            Search inbox + all folders
  folders                   List folders this workspace has created

ORGANIZING YOUR INBOX
  K2SO doesn't impose a folder taxonomy. Organize your inbox the way
  you'd organize email — create folders that fit your workflow:
    inbox move <id> projects/    # active work items
    inbox move <id> reference/   # things to remember
    inbox move <id> issues/      # problems to address
    inbox move <id> done/        # completed work
  Folders are auto-created on first `inbox move` into them.

EXAMPLES
  k2so inbox                                  # show top-level inbox
  k2so inbox list projects                    # what's in projects/
  k2so inbox read 42                          # full text of item 42
  k2so inbox move 42 projects                 # file 42 into projects/
  k2so inbox compose --title "audit auth" --body "..."   # self-note
  k2so inbox respond 42 "thanks, looking into this"
  k2so inbox archive 42                       # done with this item
  k2so inbox search "oauth"                   # find all oauth-related

RECEIVING FROM OTHER WORKSPACES
  Other workspaces send to your inbox with:
    k2so msg <your-workspace> --inbox --title "..." --body "..."
  Their items appear in your inbox alongside your own composes.

LEGACY ITEMS
  Pre-Phase-2.1 work items at .k2so/work/{inbox,active,done}/ are
  still listable; they appear with [legacy] tags. Use `inbox move`
  or `inbox archive` to bring them into the new structure.
```

---

## `k2so msg --help`

```
k2so msg <workspace> "text" [options]

Deliver text to another workspace's agent. Two main delivery modes —
think phone call vs email:

DELIVERY MODES
  (default)                 Live delivery — spawns the recipient's
                            agent if not running, blocks until the
                            message lands. Like a phone call or IM.
                            Use for urgent or synchronous comms.
  --inbox                   Drop in recipient's inbox. They read at
                            their leisure. Like email. Async; doesn't
                            spawn anything. Use for non-urgent work.

ADVANCED: TYPED SIGNALS
  --signal <kind>           Emit a typed signal. <kind> is one of:
                              msg            Plain message (= default)
                              status         Status update (telemetry)
                              presence       Online/away/offline state
                              reservation    File path lock event
                              task-lifecycle Task started/done/blocked
                              custom         Escape hatch (with --payload)

ARGUMENTS
  <workspace>               Workspace name (preferred — see `k2so workspace list`).
                            Absolute path or project UUID also accepted.
  "text"                    Message body. Wrap in quotes if it has spaces.
                            Not required when --signal <kind> needs structured payload.

OPTIONS
  --from <name>             Sender identity (default: K2SO_AGENT_NAME or "cli")
  --title "..."             Inbox title (--inbox only)
  --body "..."              Inbox body (--inbox only)
  --priority H|N|L          Inbox priority (--inbox only; default N)
  --payload <json>          Custom signal payload (--signal custom only)

EXAMPLES
  k2so msg scout_v3 "ready when you are"                       # call (live)
  k2so msg scout_v3 --inbox --title "deploy ready" --body "..."  # email
  k2so msg scout_v3 --signal status --payload '{"text":"deploying..."}'

RELATED
  k2so inbox                Read your own inbox (recipient side)
  k2so workspace list       Find workspace names
  k2so glossary signal      Learn about typed signals
  k2so glossary inbox       Learn about the inbox model
```

---

## `k2so checkin --help`

```
k2so checkin [options]

Agent self-report. Sends a heartbeat ping, status update, or
done/blocked report.

DEFAULT
  (no flags)                Plain heartbeat ping — "I'm alive, no action".

OPTIONS
  --status "message"        Report a status update visible in the workspace
                            activity feed. Doesn't change agent state.
  --done                    Report task completion. Marks the current
                            work item as done.
  --done --blocked "reason" Report blocked state. Stops the work item;
                            requires human or peer-agent unblock.

EXAMPLES
  k2so checkin
  k2so checkin --status "still on the OAuth migration"
  k2so checkin --done
  k2so checkin --done --blocked "waiting on Stripe API access"
```

---

## `k2so workspace --help`

```
k2so workspace <subcommand> [options]

Manage workspaces (project folders K2SO knows about).

DAILY SUBCOMMANDS
  list                              Yellow pages: every workspace + status
  list --running                    Filter to live agents
  launch [<path>]                   Smart cascade: attach|wake|spawn
  profile [<path>]                  Read workspace agent's AGENT.md
  update --field <f> --value <v>    Edit workspace agent profile

LIFECYCLE (less common)
  create <path>                     Create folder + register
  open <path>                       Register existing folder
  remove <path>                     Deregister (files stay)
  triage                            Plain-text summary of pending work

`<path>` defaults to K2SO_PROJECT_PATH or PWD.

EXAMPLES
  k2so workspace list
  k2so workspace launch
  k2so workspace launch /Users/me/Projects/k2so
  k2so workspace profile
  k2so workspace update --field role --value "frontend lead"
```

---

## `k2so workspace launch --help`

```
k2so workspace launch [<path>] [options]

Smart cascade — get the workspace's agent into a running state:
  * If the agent is alive (live session exists): attach to it
  * If the agent is sleeping (registered, no session): wake it
  * If the agent is cold (no registered session): spawn fresh

Returns the agent's session id on success.

ARGUMENTS
  <path>                    Workspace path (default: $K2SO_PROJECT_PATH or PWD)

OPTIONS
  --json                    Return session metadata as JSON
  --no-attach               Spawn/wake but don't attach the current shell

EXAMPLES
  k2so workspace launch
  k2so workspace launch /Users/me/Projects/k2so
  k2so workspace launch --json
```

---

## `k2so skills --help`

```
k2so skills <subcommand> [options]

Manage documentation profiles (skills) for the workspace. Skills are
markdown files (SKILL.md) describing roles, personas, and instructions
your harness loads when spawning sub-agents.

  list                              Skill profiles in this workspace
  create <name> [--template <role>] Create a new skill profile
  remove <name>                     Delete a skill profile
  profile <name>                    Read the skill's SKILL.md/AGENT.md
  regenerate [<name>]               Refresh SKILL.md files

SKILLS vs AGENT
  An agent IS a workspace (1:1). A skill is a documentation profile
  the agent (or your harness) can reference when applying a specific
  role to work.

  Spawning sessions pre-loaded with a skill is your harness's job
  (Claude Code's sub-agent spawning, Cursor's worktree management).
  K2SO no longer provides a `delegate` verb — see `k2so help-deprecated`.

  Templates (master definitions) live under `.k2so/agent-templates/<role>/`.
  Instantiated skills live under `.k2so/agents/<name>/` (historical naming;
  rename to `.k2so/skills/` planned for Phase 3).

EXAMPLES
  k2so skills list
  k2so skills create backend-eng --template rust-eng
  k2so skills profile backend-eng
  k2so skills regenerate
```

---

## `k2so settings --help`

```
k2so settings [options]

Show or modify workspace settings. Without flags: prints current settings.

WORKSPACE MODE
  --mode <off|agent|manager>
                            off: K2SO ignores this workspace
                            agent: workspace has a primary agent
                            manager: workspace coordinates other workspaces

WORKSPACE STATE (capability tier)
  --state <build|managed|maintenance|locked>
                            Drives which actions the agent can take
                            autonomously. See `k2so glossary state`.

GLOBAL TOGGLES
  --agentic <on|off>        Enable/disable all background agent systems

COMPANION (mobile + remote-desktop access)
  --companion <on|off>      Enable/disable companion server (ngrok tunnel)
  --companion-password "<pw>" Set companion auth password

EXAMPLES
  k2so settings                              # show current
  k2so settings --mode manager
  k2so settings --state managed
  k2so settings --agentic off
  k2so settings --companion on
```

---

## `k2so help-deprecated`

```
K2SO retired verbs → new equivalents

HARD-DEPRECATED (verb removed; commands exit non-zero with pointer)

  agentic on|off                  → settings --agentic <on|off>
  state list|get|set              → settings --state <id>
  mode off|agent|manager          → settings --mode <value>
  app-update                      → update --app
  commit-merge                    → commit --merge
  companion                       → daemon companion
  whatsnew                        → whats-new
  roster                          → who
  feed                            → activity
  signal <target> <kind> <payload> → msg <workspace> --signal <kind> [...]
  work send <ws> --title --body   → msg <ws> --inbox --title "..." --body "..."
  status "msg"                    → checkin --status "msg"
  agents reap                     → daemon reap
  agents triage                   → workspace triage
  delegate <agent> <file>         → Your harness handles worktree+spawn now
                                     (Claude Code sub-agent, Cursor worktree,
                                     Tauri Cmd+T). K2SO no longer manages
                                     the spawn lifecycle. For skill profile
                                     content, use `k2so skills profile <name>`
                                     and load it into your harness.

SOFT-DEPRECATED (verb still works; emits warning to stderr, forwards to new form)

  agents create                   → skills create
  agents delete                   → skills remove
  agents list                     → skills list (or `workspace list` for all)
  agents work <n>                 → inbox list <skill-name> (now inbox-keyed)
  agents lock <n>                 → (removed; skills are docs, not sessions)
  agents unlock <n>               → (removed; same reason)
  agents launch <n>               → Your harness's sub-agent spawn
  agents profile <n>              → skills profile <n>
  agents status <n>               → (removed; skills are docs, not sessions)
  agents running                  → workspace list --running

  work create --title --body      → inbox compose --title --body
  work inbox                      → inbox (default action)
  work move --from <a> --to <b>   → inbox move <id> <folder>
  work done                       → inbox archive <id>

  --agent <name> flag             → --workspace <path>   (everywhere)
                                    Old flag still works for one release.

EXAMPLES OF MIGRATION

  # Sending a message
  Old: k2so msg --agent scout_v3 "hello"
  New: k2so msg scout_v3 "hello"            # workspace-implicit; --agent removed

  # Sending a typed signal
  Old: k2so signal scout_v3 status "deploying"
  New: k2so msg scout_v3 --signal status --payload '{"text":"deploying"}'

  # Queueing work to another workspace
  Old: k2so work send my_other_ws --title "task" --body "do this"
  New: k2so msg my_other_ws --inbox --title "task" --body "do this"

  # Creating your own task / note
  Old: k2so work create --title "audit auth" --body "..."
  New: k2so inbox compose --title "audit auth" --body "..."

  # Marking a task done
  Old: k2so done --blocked "Stripe API access"
  New: k2so checkin --done --blocked "Stripe API access"   (or just `done`)

  # Listing tasks
  Old: k2so work inbox
  New: k2so inbox                            # default action lists inbox

  # Filing a completed item
  Old: k2so work move --file <f> --from active --to done
  New: k2so inbox archive <id>               # archives by id (folder-agnostic)

  # Creating a sub-agent
  Old: k2so agents create backend-eng
  New: k2so skills create backend-eng        # skill is documentation; your
                                             # harness spawns the agent

  # Spawning a sub-agent on a work item
  Old: k2so delegate backend-eng .k2so/agents/backend-eng/work/inbox/task.md
  New: Use your harness's sub-agent feature (Claude Code, Cursor) to spawn
       a worktree-based session. Reference the skill via
       `k2so skills profile backend-eng` and load into the harness's context.
```

---

## `k2so heartbeat --help`

```
k2so heartbeat [<family> <subcommand>]

Manage workspace-scoped scheduled wakes.

DEFAULT (no args)
  k2so heartbeat                                Lists active heartbeat schedules
                                                (= `heartbeat schedule list`)

THREE FAMILIES:

SCHEDULE (CRUD on heartbeat definitions)
  heartbeat schedule add --name <n> <spec>
       <spec> is one of:
         --daily --time HH:MM
         --weekly --days mon,wed,fri --time HH:MM
         --monthly --days 1,15 --time HH:MM
         --yearly --months jan,jul --time HH:MM
         --hourly --start 09:00 --end 17:00 --every 30 --unit minutes
  heartbeat schedule list [--archived]
  heartbeat schedule remove <n> [--purge]
  heartbeat schedule unarchive <n>
  heartbeat schedule edit <n> <new-spec>
  heartbeat schedule rename <old> <new>
  heartbeat schedule enable <n>                  Resume scheduling
  heartbeat schedule disable <n>                 Pause scheduling

SIGNAL (immediate one-shot triggers)
  heartbeat signal fire <n>      Fire one heartbeat now (skips schedule window)
  heartbeat signal wakeup <n>    Print/edit the WAKEUP.md
  heartbeat signal wake          Auto-wake (no name needed)

INSPECTION (read-only)
  heartbeat show <n> [--json]    Single heartbeat details
  heartbeat status <n> [-n N]    Recent fire history
  heartbeat log [-n N]           Scheduler decisions

EXAMPLES
  k2so heartbeat schedule add --name deploy-check --daily --time 09:00
  k2so heartbeat schedule list
  k2so heartbeat schedule disable deploy-check
  k2so heartbeat signal fire deploy-check
  k2so heartbeat log
```

---

## End of mock (revised)

What's modeled above:
- 14 daily verbs grouped by intent (TALK / INBOX / CHECKIN / WORKSPACE / REVIEW / ACTIVITY / INFO + ENVIRONMENT)
- 6 power-user verbs (`help --advanced`); skills surface simplified per A23
- 4 internal verbs (`help --internal`)
- 16-term glossary + per-term lookups for inbox, skill, agent, heartbeat, worktree
- Full `--help` for the load-bearing verbs (msg, inbox, checkin, workspace, workspace launch, skills, settings, heartbeat)
- `help-deprecated` showing every retired verb → new equivalent, with concrete before/after migration examples including `delegate` → harness handoff and the full `work *` → `inbox *` mapping

What's NOT modeled:
- Daemon API (`/cli/*` route shapes) — invisible to CLI users
- Implementation details — this is the user-facing surface only
- Per-flag help for every flag — sample is representative
