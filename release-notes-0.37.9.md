## Highlights

**Apple Dictation now works in K2SO terminals.** Press Fn-Fn (or your configured Dictation shortcut) inside any v2 terminal pane; the indicator anchors at the cursor cell, and words stream into the prompt as you speak.

Plus a new **Permissions settings page**, a **fix for "Too many open files"** errors that surfaced when running many panes, and a developer-only **Dictation Lab** for diagnosing input/dictation interactions.

## Apple Dictation in the terminal

Three things had to land together:

1. **`NSMicrophoneUsageDescription`** in `Info.plist` + **`com.apple.security.device.audio-input`** entitlement, signed into every release build via `release.sh`. macOS denies all audio access (including the underlying STT service Apple Dictation uses) to hardened-runtime-signed apps without these — same gate that affects `webkitSpeechRecognition` and `getUserMedia`.

2. **Native `Edit → Start Dictation…` menu item.** macOS auto-injects this when the app's Edit submenu is titled literally `"Edit"` *and* doesn't already contain a custom item with the same name. Pre-0.37.9 we had our own custom `MenuItem::with_id("start-dictation", ...)` that was suppressing the auto-inject. Removing it lets macOS wire up the real `startDictation:` selector, which Fn-Fn actually targets.

3. **Cursor-following shadow `<textarea>`** sibling to the visible terminal grid (`xterm.js`'s `_syncTextArea` pattern). One cell wide, one row tall, `opacity: 0`, positioned at the visible cursor each render. AppKit's `firstRectForCharacterRange:` query lands on a coherent on-screen rect, so the dictation indicator anchors at the cursor and engagement doesn't time out. While composition is in flight, the rect is frozen so AppKit isn't chasing a moving target.

### Streaming text into the PTY as you speak

Apple Dictation delivers a progressive best-guess transcript via `compositionupdate` events — `"hello"` → `"hello world"` → `"hello world, how"`. We type each guess straight into the PTY, prefixed by `\x7f` (DEL) bytes to walk back the previous partial. The user sees:

- Words appear in the prompt as they speak
- Cursor advances naturally (visual + behavioral)
- Mic indicator stays anchored to the cursor (because it tracks the focused textarea's rect, which we keep at the cursor)
- Brief flicker when Dictation autocorrects on commit (compositionend) — typical word, ~5 backspaces + retype

Final reconciliation at `compositionend` so Dictation's on-stop autocorrection (`"their"` → `"there"`) gets applied.

## "Too many open files" — `RLIMIT_NOFILE` raised on startup

macOS apps launched via `open` or LaunchAgent inherit launchd's default soft fd limit (256 or 1024 depending on macOS version), **not** the user's shell `ulimit -n`. K2SO's PTY pairs + WS sockets + file watchers add up fast: 10–20 active panes can saturate the limit, causing:

> `Alacritty v2: spawn failed after 12s: v2 spawn failed: Too many open files (os error 24)`

New `k2so_core::raise_nofile_limit()` helper called at the top of both the Tauri main binary and the daemon binary. Reads current `RLIMIT_NOFILE`, bumps the soft limit to the kernel hard limit (no-op when already equal). Standard pattern for daemon-style apps. Children inherit the new soft limit.

You can see it on startup in the daemon log: `[rlimit] RLIMIT_NOFILE soft raised: 256 -> 10240`.

## New: Settings → Permissions

Five rows mirroring the macOS Privacy & Security categories K2SO actually depends on:

| Permission | Why K2SO needs it | Programmatic check |
|---|---|---|
| Microphone | Apple Dictation, future voice features | `AVCaptureDevice.authorizationStatus(for:.audio)` |
| Full Disk Access | Workspace folders outside `~/Documents` (TCC-protected) | Try-read `~/Library/Safari/Bookmarks.plist` |
| Accessibility | Programmatic keystroke replay, automation tools | `AXIsProcessTrusted()` |
| Apple Events / Automation | AppleScript-driven app integration | None (open System Settings) |
| Local Network | Mobile Companion discovery on LAN | None (open System Settings) |

Polls every 2s so a freshly-granted permission flips to "Granted" without reloading. Each row's "Open settings" button deep-links to the right System Settings pane via `x-apple.systempreferences:` URLs. Microphone has a programmatic first-prompt path via `AVCaptureDevice.requestAccess`.

## New (dev only): Dictation Lab

`Settings → Dictation Lab (dev)` — hidden in production. Nine instrumented input variants (uncontrolled / controlled / lowercase-mutating / disabled-flicker / textarea / Web Speech API / etc.) side-by-side with a live event log that captures `focus` / `blur` / `input` / `composition*` / `keydown` / `keyup` / `selectionchange`. Built to isolate which input configuration breaks Apple Dictation engagement, which is how we nailed down the contributors to today's fixes.

The Web Speech API row (`webkitSpeechRecognition`) is also a working backup voice surface — programmatic mic button with streaming interim/final results. Won't break if Apple Dictation regresses again.

## Tests

759 daemon + core tests passing, same baseline as 0.37.8. No new tests added — the changes are mostly renderer-side and rely on manual smoke (which can't be unit-tested for Apple Dictation without a fake AVFoundation).

## Heads-up: still-rough edges

- **Backspace-and-retype streaming flickers visibly** when Apple Dictation autocorrects mid-flight ("hello their" → "hello there"). It's correct, but the eye-jarring. If this proves unworkable for TUI apps that interpret `\x7f` differently (some `less`/`vim` modes), we'll flip the strategy to "show transcript in an overlay, commit only at compositionend" — that path is one variable swap in `TerminalPane.tsx`.

- **Indicator placement still has macOS quirks** when the cursor is near the bottom of the pane (indicator can render above the cursor instead of below). Same behavior as every other terminal app; macOS chooses based on available screen space.

- **`Edit → Start Dictation…` menu item** auto-injects natively now; we explicitly stopped overriding it. If macOS ever stops auto-injecting in a future version, the entry will silently disappear from our menu. We'd add an explicit native NSMenuItem fallback at that point.
