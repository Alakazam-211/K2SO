# K2SO 0.35.3 — Hotfix: colors restored in Alacritty (v2)

Quick follow-up to **0.35.2**: the `claude --resume` spawn worked
again, but everything was monochrome. Claude Code's banner, your
shell prompt, `ls --color`, vim, fzf — all rendering with no
color in v2 panes.

## Root cause

When v2's daemon spawns a child PTY via `alacritty_terminal::tty::new`,
the child inherits an env where `TERM` defaulted to `dumb`. Most
TUIs check `TERM` to decide whether to emit ANSI escape sequences;
`TERM=dumb` is the universal "I don't support color" signal, so
they fall back to plain text.

The legacy renderer has always set this explicitly in
`alacritty_backend.rs:332-334`:

```rust
pty_options.env.insert("TERM".to_string(), "xterm-256color".to_string());
pty_options.env.insert("TERM_PROGRAM".to_string(), "K2SO".to_string());
pty_options.env.insert("COLORTERM".to_string(), "truecolor".to_string());
```

v2's `daemon_pty.rs` shipped without those three lines. Children
got `TERM=dumb` and politely turned colors off. Adding the three
entries to v2's `pty_options.env` mirrors legacy and restores
parity.

## Fix

Three env entries added to `crates/k2so-core/src/terminal/daemon_pty.rs::DaemonPtySession::spawn`:

| var | value | purpose |
|---|---|---|
| `TERM` | `xterm-256color` | base terminal capability advertisement |
| `COLORTERM` | `truecolor` | hints 24-bit color support to programs that look for it |
| `TERM_PROGRAM` | `K2SO` | identifies us to programs that key behavior off the host (e.g., iTerm-style detection) |

Each is added via `entry().or_insert_with()`, so callers that
explicitly set their own values (test fixtures, future heartbeat
specifics) override our defaults cleanly.

## Verification

End-to-end probe via the daemon's `/cli/sessions/v2/spawn`
endpoint, asking the child to echo its env:

```
TERM=xterm-256color
COLORTERM=truecolor
TERM_PROGRAM=K2SO
```

Claude Code's banner now renders in full color again.

## Why we missed it

Same shape of testing gap as 0.35.0 / 0.35.1: 379 unit tests cover
library logic but don't simulate "what env does a daemon-spawned
child actually inherit?" The PATH-enrichment regression test added
in 0.35.1 caught that specific class of bug; we'd need an analogous
"verify spawned child env contains TERM/COLORTERM" assertion. Filed
alongside the broader end-to-end-spawn-probe follow-up.

Tests: still 381 (291 + 15 + 75 — all pass; no test added in this
hotfix because the fix is a 3-line literal env insert, easier to
verify by inspection than to wrap in a probe).
