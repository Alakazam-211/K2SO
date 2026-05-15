# K2SO 0.35.1 — Hotfix: spawning user-installed tools

Hotfix for **0.35.0**: in production installs, opening a tab whose
command was a user-installed tool — `claude`, `cursor-agent`,
`gemini`, anything in `~/.local/bin` / `/opt/homebrew/bin` /
`/usr/local/bin` — failed with:

```
v2 spawn failed: Failed to spawn command 'claude':
No such file or directory (os error 2)
```

## What was happening

macOS's launchd starts `K2SO.app` and the `k2so-daemon` sidecar
with a deliberately sparse PATH (`/usr/bin:/bin:/usr/sbin:/sbin`).
Unlike a Terminal session, launchd does **not** source your
`.zshrc` / `.bash_profile`, so the prefixes you've configured for
your tools never make it into K2SO's environment. When the daemon
calls `posix_spawn("claude", ...)` directly, the kernel can't find
the binary.

The Alacritty (Legacy) renderer had the same gap — it just rarely
surfaced because most users invoke `claude` by typing it into an
already-running shell session, where shell rc files have already
done their PATH-enrichment work.

## The fix

Both processes (Tauri app and daemon) now adopt the user's login
shell PATH at startup. `k2so_core::enrich_path_from_login_shell`
runs the user's `$SHELL` once with `-lc 'printf %s "$PATH"'`,
adopts the result, and is done. Children inherit the rich PATH
through normal `posix_spawn` semantics — no per-spawn shell
wrapper, no per-call lookup. ~30-50ms one-time startup cost,
silent fallback if the shell exec fails.

This is the standard macOS-GUI-app pattern (used by VS Code,
Atom, Tower, GitHub Desktop, basically any `.app` that needs to
spawn user-installed binaries). The "heart" of the fix, not a
workaround — the actual problem is "this macOS process needs the
user's PATH," and the fix puts the user's PATH on the process.

## Tests added

Two regression tests in `k2so-core`:
- `enrich_path_widens_sparse_launchd_default` — paves `PATH` to
  the exact launchd default this incident hit, calls the helper,
  asserts the result is wider.
- `enrich_path_is_idempotent_on_already_rich_path` — calls the
  helper twice, asserts state is stable.

Total test count: 381 (up from 379).

## Upgrade behavior

If you installed 0.35.0 already, the auto-updater will deliver
0.35.1 in the usual way. The version-mismatch auto-restart we
shipped in 0.35.0 itself fires here — when 0.35.1's Tauri starts
and finds the launchd-held 0.35.0 daemon still running, it
kickstarts launchd's `com.k2so.k2so-daemon` so the daemon
binary refreshes in-place. No manual "Settings → Restart Daemon"
click required.

## What stays the same

Nothing else changed — same Alacritty_v2 renderer, same v2-perf
instrumentation, same selection-tracks-scroll, same WS Close-frame
hardening, same kessel-t0 archive layout. This is a one-line
behavioral fix plus regression tests.
