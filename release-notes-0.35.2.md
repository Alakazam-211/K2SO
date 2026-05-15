# K2SO 0.35.2 — Hotfix #2 for the launchd-PATH gap

The fix shipped in **0.35.1** still left `~/.local/bin` (and other
user-configured prefixes) out of the captured PATH for users whose
`~/.zshrc` is where their PATH augmentations live. v0.35.0's spawn
error came back. This release fixes it for real.

## The miss in 0.35.1

`enrich_path_from_login_shell` invoked the user's shell with
**`-lc`** (login + non-interactive). On zsh, `-l` sources
`~/.zshenv`, `~/.zprofile`, and `~/.zlogin` — but **NOT
`~/.zshrc`**. zsh only sources `.zshrc` for *interactive* shells.

Many users (myself included) put their user-bin-dir prepends in
`.zshrc`:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

So 0.35.1's helper *did* run and *did* set PATH on the daemon —
just to a PATH that was missing the very dirs we needed. Hence
`claude` (at `~/.local/bin/claude`) still wasn't findable.

## The fix

One character change: **`-lc`** → **`-ilc`**. The `-i` flag tells
zsh to behave as interactive too, which makes it source `~/.zshrc`.
Now the captured PATH matches what a real interactive terminal
sees — including `~/.local/bin`, `~/.bun/bin`, `~/.cargo/bin`, npm
globals, and anything else the user adds in `.zshrc`.

Two ancillary tweaks went in alongside:

- Stderr from the rc-source pass is now redirected to `/dev/null`
  so noisy plugins (oh-my-zsh, p10k, etc.) don't leak warnings
  into the daemon's stderr log.
- The captured PATH is read from the *last non-empty line* of
  stdout, in case the user's rc files print a banner or other
  text before our `printf %s "$PATH"` payload.

## Tests updated

- `enrich_path_widens_sparse_launchd_default` — unchanged (still
  paves PATH down to launchd default, calls the helper, asserts
  the result widens).
- `enrich_path_safe_to_call_multiple_times` — replaces the strict
  idempotency test from 0.35.1. Some `.zshrc` files reorder PATH
  dirs across invocations, so exact-equality after two calls was
  the wrong assertion. Production calls the helper exactly once;
  the test now verifies "safe to call multiple times without
  crashing or zeroing PATH." Total: still 381 tests.

## Why we missed it twice

Even with the unit tests in 0.35.1, this slipped through for the
same reason as the original 0.35.0 miss: `cargo test` runs in a
shell-rich PATH where the `widens` assertion passes regardless
of which rc files the helper actually sources. The test catches
"does enrich do anything" but not "does enrich pick up *all*
the user's PATH augmentations."

End-to-end coverage — boot the actual binary under launchd and
probe `/cli/sessions/v2/spawn` with a `~/.local/bin` tool — would
catch this class of bug, and is filed as a follow-up. Won't ship
in this hotfix.

## Upgrade behavior

Same as 0.35.1: the auto-updater delivers the new binary, and the
0.35.0 version-mismatch auto-restart fires when 0.35.2 boots and
finds the launchd-held 0.35.1 daemon still running.
