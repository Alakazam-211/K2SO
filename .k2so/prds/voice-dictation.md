# PRD: Voice Dictation in Terminal Panes (Hidden Shadow Input)

## Problem

macOS Apple Dictation (Fn-Fn or Globe key) silently does nothing when
the user has a K2SO terminal pane focused. The Dictation feature only
fires when the focused element is a standard text input that AppKit
recognizes — `NSTextField`, `NSTextView`, or a WebView element AppKit
can resolve as one of those (`<input>`, `<textarea>`, or
`contenteditable`). Tauri's WebView wraps an `NSView`, but the actual
focused element inside the WebView is K2SO's terminal container — a
`<div tabIndex={0}>` (`TerminalPane.tsx::containerRef`) that AppKit
sees as a generic non-text element.

The user's reasonable expectation is "speak into the terminal." The
actual experience is "press Fn-Fn, no dictation indicator appears,
no audio gets captured, no error surfaces — silent failure."

This is a known class of WebView problem. iTerm2, Terminal.app, Warp,
and Hyper all solve it the same way: place an invisible focusable
text input on top of the active pane, route its key + composition
events back into the terminal's input pipeline. macOS sees a real
text input, accepts dictation, and the dictated text streams to the
PTY.

K2SO currently has no equivalent. Dictation is the most user-visible
gap, but the same hidden-input is also the right surface for IME
composition (CJK candidate window, accent picker, emoji palette) —
all of which today either misbehave or don't trigger at all in the
WebView terminal.

## Goal

Land a per-pane invisible focusable `<textarea>` that AppKit treats
as a real text input, paired with a focus-management dance that
routes its keystrokes + dictation results back to the v2 PTY via
the existing `sendInput(text)` WS write at
`TerminalPane.tsx:1022-1026`. Visible behavior is identical to today
for typed input; pressing Fn-Fn now triggers macOS Dictation, and
dictated text streams to the terminal as it would for any other
text input.

Specifically:

1. Apple Dictation works in any v2 terminal pane (chat, agent, free
   PTY) without UI changes the user has to enable.
2. Typed input is byte-identical to today: every escape sequence
   `keyEventToSequence` produces still reaches the PTY.
3. Paste behavior (including Finder file-path paste via
   `clipboard_read_file_paths`) unchanged.
4. IME composition (Japanese, Chinese, Korean, accent input) works
   correctly — composition events reach the textarea, the candidate
   window appears, and the committed string flushes to the PTY in
   one `sendInput` call.
5. Selection and link-hover (cursor → URL detection) still work on
   the visible terminal grid.

## Out of scope

- **Push-to-talk / hold-to-record buttons.** This PRD is exclusively
  about making macOS Dictation reach the terminal. A separate UI
  affordance (e.g. a microphone button in the chat tab) is its own
  PRD.
- **Local STT (whisper.cpp etc.).** K2SO ships a local LLM but does
  not run a local STT model. Dictation routes through Apple's STT
  service. If we want offline STT later, that's a separate spec.
- **TTS / speech output.** No "K2SO speaks back" in this PRD. Voice
  output is its own product question (which voice, when does it fire,
  does it interrupt, etc.).
- **Kessel renderer.** Kessel is alpha and not the default. Dictation
  in Kessel is the same WebView problem and would use the same fix,
  but we ship v2 first; Kessel migration carries the fix forward
  whenever Kessel is mainlined.
- **Windows / Linux dictation.** Different OS-level surface; out of
  scope for the macOS-first ship. If Windows users need dictation
  later, the shadow-input approach generalizes (Windows speech
  recognition has the same "must be a real text input" requirement),
  but call it explicitly when we get there.
- **Per-character dictation streaming into running TUI input
  widgets** (claude prompt, vim insert mode, etc. seeing each word as
  it's spoken). Dictation commits text on a phrase boundary; we
  forward whatever Apple delivers via the textarea's `input` event,
  same as paste. If a user wants real-time speech in claude, claude's
  TUI receives the dictated text the moment Apple commits it.
- **Voice commands to K2SO itself** ("hey K2SO, open the chat tab").
  Different surface (system-wide hotword), different scope.

## Background — what's there today

`TerminalPane.tsx` is the v2 terminal pane component
(`src/renderer/terminal-v2/TerminalPane.tsx`, ~1,400 lines).

Relevant existing structure:

| Element | Purpose | Today |
|---|---|---|
| `containerRef` (`<div tabIndex={0}>`) | The focused element. | Receives `keydown`, `paste`, drag-drop. |
| `sendInput(text: string)` | WS write helper. | Sends `{action:'input', text}` to daemon → PTY. |
| `naturalTextEditingSequence` | Maps Cmd+arrow / Cmd+backspace / etc. to readline-equivalent escapes. | Runs on every `keydown`. |
| `keyEventToSequence` | Maps regular keys + modifiers to xterm sequences. | Runs on every `keydown`. |
| `onPaste` | Reads `clipboardData`, falls back to `clipboard_read_file_paths` for Finder paths. | Bypasses keymap; `sendInput`s the resolved text directly. |
| Visible rows | `<div>`s rendered from `snapshot.scrollback + grid`. | Selection-anchored to row text nodes. |

Auto-focus logic (`TerminalPane.tsx:976-1003`):

- On mount: `containerRef.focus()` via `requestAnimationFrame`.
- On window-regain-focus: re-focus if `containerRef` was the active
  element before blur (avoids stealing focus from a sidebar input).

Send path:

```
keyDown event → onKey → keyEventToSequence → sendInput → WS → daemon → PTY
                                                                       ↓
                                                                v2_session_map
                                                                       ↓
                                                                 child stdin
```

There's no other input surface — no contenteditable, no textarea, no
NSTextView in the pane.

## Approach — hidden shadow textarea

Insert a `<textarea>` into each `TerminalPane` that:

- Is invisible to the user (zero opacity, zero size, but still in
  the layout — not `display:none`, since hidden elements don't
  receive focus).
- Is the focused element when the pane is active (replacing the
  current `containerRef.focus()` call).
- Forwards all keystrokes back to the existing `onKey` handler so
  `keyEventToSequence` etc. still drive the PTY.
- Listens for `input` events (where dictation, IME commits, and
  paste actually deliver text) and `sendInput`s the delta to the
  PTY.
- Listens for `compositionstart` / `compositionupdate` /
  `compositionend` so IME candidate windows work correctly.
- Has its value cleared after every commit so it doesn't accumulate.

The visible terminal grid stays exactly as today. The textarea sits
on top in the layout (or off-screen — see "Layout choices" below)
but doesn't paint.

This is the same trick iTerm2, Terminal.app, Warp, Hyper, and VS
Code's terminal use. Battle-tested, predictable behavior.

### Why not contenteditable on the canvas

(Approach #2 from the original triage.) `contenteditable` works for
dictation but rewires:

- Selection (browsers handle selection differently in editable vs
  non-editable trees)
- Native paste handling (browsers run paste through the document's
  edit machinery, which inserts HTML by default)
- IME composition into the rendered grid (composition characters
  paint into the grid as text nodes the user can see, then have to
  be unwound)

The risk surface is large, and our existing keymap + `naturalText
EditingSequence` handler would need to be adapted to a different
event model. The shadow textarea isolates the textiness.

### Why not menu-only

(Approach #3.) A `Edit → Dictate Message` menu item that pops a
sheet works for the workflow but breaks the user's intuition that
Fn-Fn dictates wherever the cursor is. iTerm2 does NOT do this for
a reason. We can still ship a menu item later as a discoverability
aid, but the shadow-input is the architectural answer.

## Layout choices

The textarea must:

1. Be focusable (`autoFocus` allowed; `tabIndex >= 0`).
2. Be in the visual layout (not `display:none` or
   `visibility:hidden`, both of which make the element
   non-focusable in WebKit).
3. Not be visible to the user.
4. Not eat pointer events (so click-to-place-cursor still works on
   the terminal grid for selection).

Options:

| Layout | Notes |
|---|---|
| **A. Off-screen** (`position:absolute; left:-9999px; top:-9999px`) | Old-school screen-reader-friendly hide. Loses focus rings around the actual pane (fine, we don't render them anyway). Works in WebKit. |
| **B. On-pane, transparent** (`position:absolute; inset:0; opacity:0; pointer-events:none`) | Still focusable. Click-through works because of `pointer-events:none`. The textarea overlays the visible grid invisibly. |
| **C. 1×1 pixel inside the container** | Simplest. AppKit cares about element type, not size. |

**Recommendation: B (on-pane transparent).** Off-screen layouts can
trip macOS accessibility heuristics; keeping the textarea inside the
pane's bounds with `pointer-events:none` is the closest analog to
how `contenteditable` would behave, minus the selection/IME side
effects.

## Implementation

### Phase 1 — Per-pane shadow textarea

`TerminalPane.tsx`:

```tsx
const shadowInputRef = useRef<HTMLTextAreaElement | null>(null)

// (existing containerRef stays, but is no longer the focus target.)

return (
  <div ref={containerRef} className="terminal-pane" /* ... */>
    {/* visible terminal grid — unchanged */}
    <div className="terminal-grid">{/* rows */}</div>

    {/* invisible focusable shadow input — Phase 1 */}
    <textarea
      ref={shadowInputRef}
      // Apple Dictation needs a real <textarea> AppKit can resolve
      // through the WebView. See PRD: voice-dictation.md.
      autoCorrect="off"
      autoCapitalize="off"
      spellCheck={false}
      // CJK/IME composition fires here.
      onCompositionStart={onComposeStart}
      onCompositionUpdate={onComposeUpdate}
      onCompositionEnd={onComposeEnd}
      // Dictation commits, IME commits, and paste deliver text via
      // 'input'. Read the delta, sendInput, then clear.
      onInput={onShadowInput}
      // Forward all keystrokes back to the existing onKey handler
      // so keyEventToSequence drives the PTY exactly as today.
      onKeyDown={onKey}
      // Block native paste; we route through clipboard_read_file_paths.
      onPaste={onPaste}
      className={SHADOW_INPUT_CLASS}
      aria-hidden="true"
    />
  </div>
)
```

`SHADOW_INPUT_CLASS` (Tailwind):

```
absolute inset-0 opacity-0 pointer-events-none
resize-none border-0 outline-none
```

Auto-focus moves from `containerRef.focus()` to
`shadowInputRef.current?.focus()` in:

- Mount auto-focus (`useEffect` at line ~976).
- Window-regain-focus handler (line ~999).
- The `el.focus()` inside the keydown effect (line 1079).

The visible focus state of the pane (highlight border, etc.) keys
on `document.activeElement === shadowInputRef.current` rather than
the container.

### Phase 2 — `input` event handler (the dictation delivery)

```tsx
const composingRef = useRef(false)

const onComposeStart = () => { composingRef.current = true }
const onComposeUpdate = () => { /* IME candidate showing — no PTY write */ }
const onComposeEnd = (e: CompositionEvent) => {
  composingRef.current = false
  const committed = e.data
  if (committed) {
    setViewportOffset(0)
    sendInput(committed)
  }
  // Clear so the next dictation/IME pass doesn't accumulate.
  if (shadowInputRef.current) shadowInputRef.current.value = ''
}

const onShadowInput = (e: React.FormEvent<HTMLTextAreaElement>) => {
  // During IME composition, do NOT forward — `compositionend`
  // commits the final string. Forwarding mid-composition leaks
  // candidate characters to the PTY.
  if (composingRef.current) return
  const ta = e.currentTarget
  const text = ta.value
  if (text.length === 0) return
  setViewportOffset(0)
  sendInput(text)
  ta.value = ''
}
```

Apple Dictation, paste, and IME-final all fire `input` (paste also
fires `paste` first, but `input` runs after — we use the existing
`onPaste` to short-circuit, then ignore the redundant `input` since
the textarea was cleared by `onPaste`'s `e.preventDefault()`).

### Phase 3 — Key event routing

The existing `onKey` handler at `TerminalPane.tsx:1040-1053` runs
through `naturalTextEditingSequence` and `keyEventToSequence` and
calls `e.preventDefault()`. When the textarea hosts the keydown
event:

- `e.preventDefault()` blocks the textarea from inserting the
  character into its own value — which is what we want (we send the
  byte ourselves; the textarea stays empty).
- For modifier-only keys (Cmd, Shift), the existing path returns
  `null` from `keyEventToSequence` and falls through. The browser
  default for those is harmless (no character inserted).

One extra rule: when `keyEventToSequence` returns non-null, we
already preventDefault. Add a check that
`composingRef.current === false` at the top of `onKey` — during IME
composition, return early without preventDefault so the textarea
absorbs the IME keystrokes.

### Phase 3.5 — `Edit → Start Dictation` menu item

Discoverability gap: users who haven't memorized the Fn-Fn shortcut
won't know K2SO supports dictation. Adding a menu entry under the
Edit menu surfaces the capability and gives keyboard-shortcut-averse
users a click path.

**Tauri menu definition** (Rust side, `src-tauri/src/menu.rs` or
wherever the Edit submenu lives):

```rust
// Edit menu
.text("start_dictation", "Start Dictation")
// macOS Dictation has no canonical accelerator across machines (Fn-Fn
// vs Globe vs custom), so don't bind one. The menu item is a click
// affordance; the system shortcut continues to work independently.
```

**Tauri command** (Rust side):

```rust
#[tauri::command]
pub fn start_dictation(window: tauri::Window) -> Result<(), String> {
    // Tell the renderer's active pane to focus its shadow input,
    // then fire the system "start dictation" service. The service
    // is what AppKit invokes when the user presses Fn-Fn — going
    // through the same path means the indicator UI, audio capture,
    // and commit semantics are identical.
    window.emit("k2so://start-dictation", ()).map_err(|e| e.to_string())
}
```

The command emits an event the renderer listens for. On the renderer
side:

```ts
// src/renderer/main.tsx (or wherever Tauri events are wired)
import { listen } from '@tauri-apps/api/event'

listen<void>('k2so://start-dictation', () => {
  // The active pane's shadow input is already focused. Trigger
  // the system Dictation service via a synthetic NSPerformService
  // call. WebKit doesn't expose this directly — we round-trip
  // through a one-line Objective-C helper exposed as a Tauri
  // command (see `src-tauri/src/services/dictation.rs`).
  invoke('start_system_dictation').catch(() => { /* non-fatal */ })
})
```

**Native helper** (`src-tauri/src/services/dictation.rs` —
~15 lines of objc2/cocoa):

```rust
#[tauri::command]
pub fn start_system_dictation() -> Result<(), String> {
    // NSPerformService("com.apple.Dictation", null) is what AppKit
    // calls when the user activates Dictation. The service binds to
    // the focused responder, which is our shadow textarea — same
    // outcome as Fn-Fn.
    use objc2_foundation::NSString;
    use objc2_app_kit::NSPerformService;
    unsafe {
        let service = NSString::from_str("com.apple.Dictation");
        NSPerformService(&service, std::ptr::null_mut());
    }
    Ok(())
}
```

(If `NSPerformService` proves flaky across macOS versions, fallback
is to run `osascript -e 'tell application "System Events" to keystroke "..."`
with the user's configured Dictation shortcut — but that requires
Accessibility permission. Try the service-based path first.)

**Tests:**

- Unit: clicking the menu item emits the `k2so://start-dictation`
  event (Tauri test harness).
- Manual: click `Edit → Start Dictation` with no terminal pane
  focused → command fails gracefully (no crash, log line). With a
  terminal pane focused → Dictation engages, indicator appears,
  speech captures.

**Why a menu item over a button in the chrome:** matches macOS
conventions (`Edit → Start Dictation` is where every other native
app puts it — TextEdit, Notes, Pages). A floating mic button would
be K2SO-unique and clutter the chrome. The menu also auto-shows the
user's configured Dictation shortcut next to the entry, which
doubles as documentation for "press THIS key combo next time."

### Phase 4 — Tests

Renderer-side unit tests under
`src/renderer/terminal-v2/__tests__/`:

1. `shadow_input_receives_focus_on_mount` — render a `TerminalPane`,
   assert `document.activeElement === shadowInputRef.current` after
   mount.
2. `shadow_input_keystroke_routes_to_send_input` — fire a `keydown`
   for `'a'` on the textarea, assert `sendInput` called with the
   expected sequence (matching today's behavior).
3. `shadow_input_paste_uses_existing_pipeline` — fire a `paste`
   with a text clipboard, assert `sendInput` called with the
   pasted text.
4. `shadow_input_dictation_simulates_via_input_event` — programmatic
   `change` + `input` event with `value="hello world"`, assert
   `sendInput("hello world")` called once and the textarea cleared.
5. `shadow_input_ime_composition_holds_until_end` — fire
   `compositionstart`, `compositionupdate`, `compositionend` with
   final value `"日本語"`, assert no `sendInput` during update,
   exactly one `sendInput("日本語")` on `compositionend`.

End-to-end manual test (dictation can't be unit-tested without
faking AVFoundation):

- Open a chat tab in a v2 workspace.
- Press Fn-Fn (or whatever Dictation shortcut the user has).
- Speak a phrase.
- Confirm: dictation indicator appears, text streams into the
  terminal (visible at the prompt), `Enter` to commit.

### Phase 5 — Acceptance criteria

A. **Dictation works.** Fn-Fn while a v2 pane is focused triggers
   macOS Dictation. Spoken text reaches the PTY.
B. **No regression in keyboard input.** Every keymap test today
   still passes against the new shadow-input pathway. Cmd+C copies
   selection. Cmd+V pastes (including Finder file paths). Arrow
   keys, Cmd+arrow, Ctrl+letters, function keys all produce the
   same bytes.
C. **No regression in IME.** Japanese, Chinese, Korean candidate
   windows work and commit correctly. Accent picker (long-press a
   key) works.
D. **No regression in selection / link hover.** Mouse drag selects
   text on the visible grid. Hovering a URL still highlights.
E. **No regression in scroll/wheel.** Mouse wheel still scrolls
   the buffer. Pinch-zoom still adjusts font size if that's wired.
F. **No regression in tab focus.** Cmd+T opens a new tab and the
   new tab's shadow input takes focus. Closing a tab returns focus
   to the prior tab's shadow input.
G. **Window blur/regain.** Switching apps and returning re-focuses
   the shadow input on the previously-focused pane (current
   `window.focus` handler ports to the new ref).
H. **Menu item engages Dictation.** `Edit → Start Dictation`
   activates macOS Dictation against the focused pane — same
   indicator, same capture, same commit behavior as Fn-Fn. The
   menu entry shows the user's configured Dictation shortcut next
   to the label.

### Phase 6 — Rollout

Single-shot ship — there's no migration. Either the shadow input is
present and dictation works, or it isn't. Phasing for risk:

- **0.37.X (this work):** Phase 1-3 in `TerminalPane.tsx`. Smoke
  test all keymap paths. Ship to a 0.37.X if a release window is
  open, else 0.38.0.
- **No DB migrations.** Pure renderer change.
- **No daemon changes.** The PTY write path is unchanged — the
  shadow input is purely a renderer-side textiness wrapper.

If any keymap regression surfaces in the wild, the rollback is
trivial: revert the `TerminalPane.tsx` change, focus returns to
`containerRef`, dictation breaks again but everything else recovers.

## Risks

### A. Dictation indicator overlay collides with terminal cursor

When macOS Dictation is active, the system draws a microphone
indicator near the focused text input. With the textarea
positioned `inset-0` and invisible, the indicator may render at
the textarea's origin (top-left of the pane) instead of where the
user expects (at the terminal's prompt). Likely acceptable —
indicator is small, top-left of pane is reasonable. If it's
distracting, position the textarea at the cursor's screen position
each frame. Defer until reported.

### B. Shadow input focus traps Cmd+W / Cmd+Q

Browsers normally send Cmd+W to the WebView container, which Tauri
forwards to the menu (close tab). With keystrokes captured at the
textarea, the existing `onKey` handler must continue to return
`null` from `keyEventToSequence` for Cmd-prefixed shortcuts so the
event bubbles to the document for menu handling. This is already
the existing behavior — verify in the test plan.

### C. AppleScript / accessibility tools see the textarea instead of the pane

Tools that introspect the focused element via accessibility APIs
(Cleanshot OCR, Raycast quicklinks, etc.) will see a `textarea`
where they used to see a `div`. The `aria-hidden="true"` reduces
this somewhat, but if a tool looks past aria, it'll find an empty
textarea. Acceptable — those tools were broken on the existing
`<div>` anyway.

### D. Browser-level autocomplete suggestions appear over the terminal

WebKit may show inline autocomplete suggestions for
spell-checked or autofilled input. Mitigated by `autoComplete="off"`,
`autoCorrect="off"`, `autoCapitalize="off"`,
`spellCheck={false}`, and `data-1p-ignore` (1Password ignore). Add
all to the textarea props. If suggestions still leak, the next
mitigation is `inputMode="none"` to suppress the on-screen keyboard
suggestion bar — but that may also disable dictation. Test before
adding.

### E. Selection mid-pane while textarea has focus

If the user clicks-and-drags to select text on the terminal grid,
the textarea is `pointer-events:none`, so the click goes through
to the visible row divs. Selection works. But the textarea **also**
loses focus (because the click target was a different element).
After the selection ends, focus needs to return to the textarea
so dictation still works on the next Fn-Fn.

Add a `mouseup` handler on the container that re-focuses
`shadowInputRef.current` if the click occurred inside the pane.
Skip if the user is interacting with a child interactive element
(scroll bar, link, etc.) — we already have similar logic for the
`window.focus` handler.

### F. Initial `value=""` doesn't equal `""` on every browser

Older WebKit returned `null` from `textarea.value` until something
was typed. Should be a non-issue on modern WebKit (Tauri is
WebKit-based on macOS). Defensive: read `ta.value ?? ''`.

## Design decisions (resolved)

1. **Shadow input is per-pane, not a window-level singleton.**
   Each `TerminalPane` owns its own `<textarea>` via
   `shadowInputRef`. Mounts and unmounts with the pane. Avoids the
   focus-rebind dance a singleton would require on every pane
   switch. ~5 KB DOM per pane is negligible.
2. **Ship a menu item for discoverability:**
   `Edit → Start Dictation`. Users who haven't memorized the Fn-Fn
   shortcut get a visible affordance. The menu item programmatically
   focuses the active pane's shadow input and dispatches the same
   action AppKit fires for the system shortcut, so dictation engages
   exactly as if the user had pressed Fn-Fn. See Phase 3.5 below.
3. **Textarea is a sibling of the grid, not a child.** The visible
   grid does link-detection hit-testing against its own bounds; an
   in-grid textarea risks hovering false-positive on links. Sibling
   layer, absolutely positioned `inset-0`, `pointer-events:none`.

## Definition of done

After this ships:

1. A user with a v2 workspace open presses Fn-Fn while a terminal
   pane is focused. macOS Dictation engages. Dictated text streams
   to the PTY.
2. Every existing keymap test passes — no terminal-input regression.
3. The PRD's Phase 5 acceptance criteria A-G all pass on a manual
   smoke test against a fresh build.
4. Tests under `src/renderer/terminal-v2/__tests__/` cover the
   five Phase 4 cases.
5. Release notes call out: "Apple Dictation now works in K2SO
   terminal panes — press your Dictation shortcut and speak."

That last point is the qualitative test: a user who'd given up on
Fn-Fn in K2SO can press it tomorrow and have it just work.
