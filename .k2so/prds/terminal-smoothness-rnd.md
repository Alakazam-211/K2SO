# R&D Charter: Buttery-Smooth Terminal (Local GPU + Remote K2 Connect)

> **Status**: R&D / research charter. Not a committed implementation plan yet.
> **Companion docs**: [`docs/terminal-scrolling-research.md`](../../docs/terminal-scrolling-research.md)
> (terminal-emulator-specific findings + the A/B/C/B′ options),
> [`docs/terminal-rendering-research.md`](../../docs/terminal-rendering-research.md)
> (rendering correctness: selection, URLs, TUI, IME).
> **Provenance**: cross-vertical OSS research sweep, 5 parallel research
> fronts (cloud gaming, remote desktop, Rust/web GPU text rendering,
> transport/wire-protocol, Tauri+GPU composition & cross-industry analogs).
> All claims primary-source-verified; URLs inline.

---

## 1. Mission & the two-regime thesis

Make K2SO's terminal feel buttery smooth — **without** giving up the
foundational invariant that the **Rust daemon owns authoritative terminal
state** (`alacritty_terminal` grid + scrollback + PTY). That invariant is
what enables one-session-many-windows and **K2 Connect** (attaching to a
*remote* daemon over a tunnel).

The core realization that organizes all the research: **there are two
"smooth" problems pulling in opposite directions.**

| Regime | Bandwidth | Latency | What "smooth" needs |
|---|---|---|---|
| **LOCAL** (daemon + UI same machine) | free | ~0 | Push *pixels/GPU*; optimize draw. |
| **REMOTE / K2 Connect** (daemon over a tunnel) | constrained, lossy | real RTT | Push *semantic deltas*; optimize bytes + resilience. |

**The headline cross-vertical finding: a text grid is NOT video — and
encoding it as video (the cloud-gaming reflex) is strictly worse.** Our
existing compact CellRun-delta stream is *already on the correct side* of
the architecture for the remote case. The win is not "replace it with a
codec" — it's (a) make the local renderer GPU-fast, and (b) harden the
semantic delta stream with backpressure + scroll/cache optimizations.

**The novel thing we must invent** (nobody in the research has shipped
this exact shape): **one daemon-authoritative grid that feeds BOTH a local
GPU painter AND a remote semantic stream**, switching technique by regime,
not by rewriting the model.

---

## 2. What we already do right (keep these)

- **Daemon owns `alacritty_terminal`** — same engine as Zed/Alacritty; we
  get the ring-buffer grid, O(1) display offset, and damage tracking for
  free.
- **Damage-based CellRun deltas** (only changed rows + appended
  scrollback) — this is the *Broadway / RDP-orders / Guacamole* model, the
  good side for remote. Xpra/RDP/VNC all converge on "semantic damage, not
  pixels."
- **Client-side scroll** (no WS round-trip) — correct instinct.
- **Style-coalesced CellRuns** — same idea as Zed's BatchedTextRun.

## 3. What's actually broken (the jank, restated)

The frontend mirrors Alacritty's O(1) grid into JS and then **re-renders
it with React DOM diffing on every wheel tick** (custom DOM renderer: one
`<div>`/row, `<span>`/run; scroll = a 50ms `setTimeout` → full visible-row
re-render). File-level specifics in `docs/terminal-scrolling-research.md`
(re-verify line numbers against current `main` — terminal code changed a
lot across 0.39.6–0.39.9).

---

## 4. The decisive constraint that shapes the whole design

**WebGPU is not usable in WKWebView yet.**
- WebGPU shipped in **Safari 26.0** (Sept 2025) and in **Safari.app**, but
  **WKWebView does not expose it** — an Apple engineer confirmed there is
  *no flag/entitlement/preference* an embedding app (Tauri/wry) can set;
  it arrives only when Apple flips the WKWebView default, timeline unknown.
  ([Apple DevForums 770862](https://developer.apple.com/forums/thread/770862),
  [WebKit WWDC25](https://webkit.org/blog/16993/news-from-wwdc25-web-technology-coming-this-fall-in-safari-26-beta/))

**Consequence:** the in-webview GPU path **must target WebGL2**, not
WebGPU. That's fine — WebGL2 + `drawElementsInstanced` is exactly the
canonical terminal-painter stack (xterm `addon-webgl`). The native wgpu
path (C) sidesteps this entirely (wgpu → Metal) but inherits a hard
compositing/IME problem (§6).

Related transport gating: **WebTransport/QUIC** only reached WKWebView in
**Safari 26.4** (~baseline March 2026) — forward-looking for K2 Connect,
but binary-over-WebSocket must stay a permanent first-class fallback.

---

## 5. Recommended architecture (research consensus)

### Local renderer: **Path B′ on WebGL2** — primary recommendation
A WebGL2 painter *inside* the webview, driven by the grid deltas the
daemon already sends. Model on **`@xterm/addon-webgl`** + the **Zed
alpha-atlas** insight:
1. `<canvas>` with `getContext('webgl2')` replacing/overlaying the DOM grid.
2. **Alpha-only glyph atlas**, trimmed + multi-page; tint per-instance so
   fg/bg colors and theme changes don't multiply atlas entries (Zed's
   trick — one atlas serves all colors).
3. **One `Float32Array` instance buffer**: per cell `{col,row,atlasUV,
   fgRGBA,bgRGBA,flags}`. **Apply daemon deltas directly into changed cell
   slots** — this is the structural win over per-row `<div>`/per-run
   `<span>`.
4. Draw: background pass + glyph pass, each one `drawElementsInstanced`.
5. Emulator stays in Rust; the painter is a pure consumer. **IME +
   text-selection stay native to the webview** — the killer advantage for
   a *terminal* specifically.

**Why B′ over the alternatives:** xterm.js itself (Option B) is
eliminated — it's a full JS emulator that would own the grid client-side,
breaking daemon-authority + K2 Connect. Native wgpu (Option C) has a
higher ceiling but breaks IME/selection and hits a Linux compositing
blocker (§6). B′ is the smallest leap from today's DOM grid and clears the
WebGPU/WKWebView blocker.

Even short of a full GPU painter, **Option A** (rAF instead of 50ms
setTimeout; `transform: translateY()` scroll layer; fix snapshot-churn
memo; ring-buffer the JS scrollback mirror; ~8ms daemon coalescing) buys
~80% of the local win in days and is a strict prerequisite/forcing-function
for B′ anyway.

### Native escape hatch: **Path C (wgpu + glyphon)** — Phase-2, gated
Renderer side is well-trodden: **wgpu + `glyphon`** (or hand-rolled
instanced painter w/ Zed's alpha-atlas layout). The hard part is
**compositing with WKWebView**, which Tauri doesn't officially support.
- macOS is the *most* feasible platform (WKWebView is an `NSView`; Metal
  composites via Core Animation). Proven pattern:
  **`qwook/tauri-plugin-steam-overlay`** — a transparent click-through
  child window over an opaque wgpu child window (`set_ignore_cursor_events`
  routes input through). The flicker in
  [tauri#9220](https://github.com/tauri-apps/tauri/issues/9220) is a
  **Linux/GTK-only** problem, *not* macOS.
- **But** C breaks IME/selection over GPU pixels — you'd end up keeping a
  transparent webview overlay just for selection/IME, i.e. a B′+C hybrid.
  So C only earns its complexity if B′ hits a ceiling. Treat as escape
  hatch, prototyped against the steam-overlay child-window pattern.

### Transport
- **Local loopback**: keep the **localhost WebSocket**; swap JSON → a
  **binary frame** (postcard, or a fixed-layout `DataView` schema:
  `run = [u16 col, u16 len, u8 fg, u8 bg, u8 attrs, utf8…]`). ~2.5× smaller,
  ~3–4× faster decode than JSON. *Don't* route the delta firehose through
  Tauri `invoke`/IPC (string-serialization bottleneck); the custom URI
  protocol is request/response-shaped and not a clean duplex fit. WS wins.
- **Remote / K2 Connect**: target **WebTransport (HTTP/3+QUIC)** via Rust
  `wtransport`/`web-transport-quinn` — per-stream flow control, no TCP
  head-of-line blocking, optional unreliable-datagram lane for
  supersede-able full-grid refreshes. **Feature-detect** (`'WebTransport'
  in window`) and **fall back to the same binary-over-WS frame** (one
  format serves both — key simplification). Pre-Safari-26.4 users have no
  WebTransport, so WS fallback is permanent.

### Flow control (do this regardless of renderer — highest-leverage single change)
Our push-only stream has **no backpressure** today. Two mature systems
converged on the same answer: add an **ack-gated frame loop**.
- **Guacamole `sync`**: daemon tags each logical frame with a monotonic
  id; frontend echoes it once applied; daemon **withholds further deltas
  until the ack arrives** (with a timeout). ([protocol](https://guacamole.apache.org/doc/gug/guacamole-protocol.html))
- **RustDesk `VideoFrameController`** independently does ack-gated capture
  (`try_wait_next` blocks until `notify_video_frame_fetched`).
- The ack must propagate **all the way to the PTY**: when the client is
  behind, the daemon **pauses draining the PTY master fd** (real OS
  backpressure) — do *not* just buffer in the daemon (recreates xterm.js's
  silent **50MB-drop** failure one layer up).
- On a slow REMOTE link, make emit cadence an RTT-driven controller
  (RustDesk `VideoQoS` style): coalesce more aggressively, raise min
  inter-frame interval, drop intermediate cursor-blink frames. On fast
  LOCAL, send every frame uncoalesced.

### Scrollback bandwidth (the standout remote optimization)
- **Scroll-as-motion-vectors** (Xpra `scroll` pseudo-encoding) + **VNC
  CopyRect** + **Broadway node-cache + `GskOffsetNode` reposition** +
  **RDP glyph cache** all attack the same thing: don't re-send content
  that already exists on the client. For us: hash stable CellRuns/rows;
  on scroll emit **`copy row R from history H`** / **`reuse run #id`**
  refs instead of full payloads. Scrolling (`cat`, build logs, `less`) is
  the single biggest bandwidth event in a terminal — this makes it nearly
  free on the remote path.

---

## 6. The "one grid, two consumers" architecture we must invent

Nobody in the research ships exactly this, so it's our R&D frontier:

```
                 ┌─────────────────────────────┐
                 │  Rust daemon (authoritative)  │
                 │  alacritty_terminal grid +    │
                 │  scrollback + PTY + damage    │
                 └──────────────┬────────────────┘
                                │  ONE delta model
                                │  (CellRun damage + scroll/copy refs)
            ┌───────────────────┴───────────────────┐
            │                                         │
   LOCAL consumer                            REMOTE consumer (K2 Connect)
   binary-over-loopback-WS                   WebTransport/QUIC (WS fallback)
   → WebGL2 instanced painter (B′)           → same WebGL2 painter
   full-fidelity, every frame                + ack/credit backpressure
   uncoalesced                               + RTT-driven coalescing
                                             + scroll/copy refs (bandwidth)
                                             + FEC/unreliable lane (resilience)
```

Key design properties to prove out:
- **Same wire frame format** for both regimes; the *cadence/coalescing
  policy* differs, not the schema.
- **Same WebGL2 painter** consumes both; it never knows whether the daemon
  is local or remote.
- **Resync primitive** (the "I-frame"): a full-grid snapshot for newly
  attached windows / reconnects. Borrow Unreal's *intra-refresh* idea —
  spread a big resync across frames so attach/reconnect doesn't spike.
- **Multi-viewer bitrate negotiation**: when local + remote windows attach
  to one session, QoS must pace to the *slowest* viewer (RustDesk shares
  bitrate across clients).

---

## 7. Master repo list — ranked by research value to us

The user's core ask: *which repos benefit us most from a research
perspective.* Consolidated across all 5 fronts:

### Tier ★★★ — study deeply, direct templates
| Repo | Why it's top-tier |
|---|---|
| [**Zed GPUI**](https://zed.dev/blog/videogame) (blade→wgpu) | THE blueprint for GPU text-grid data layout: alpha-only glyph atlas, color-via-shader-multiply, 16 sub-pixel variants, batch-by-primitive-type, one instanced draw. Applies to B′ *and* C. |
| [**@xterm/addon-webgl**](https://github.com/xtermjs/xterm.js/pull/1790) | The canonical WebGL2 terminal painter — exact B′ template. Trimmed multi-page atlas, per-cell `Float32Array`, `drawElementsInstanced`. Guaranteed to run in WKWebView. |
| [**glyphon**](https://github.com/grovesNL/glyphon) | Canonical Rust/wgpu glyph painter (cosmic-text + etagere). Drop-in for Path C; one `pass.draw(0..4, 0..N)`. |
| [**GTK Broadway**](https://docs.gtk.org/gtk4/broadway.html) | Conceptually identical to our model: streams render-node diffs → browser. Steal node-cache + `GskOffsetNode` reposition (our scroll/copy-ref design). |
| [**Apache Guacamole**](https://guacamole.apache.org/doc/gug/guacamole-protocol.html) | `sync`-timestamp ack backpressure — the single highest-leverage protocol change for us. Length-prefixed streamable framing. |
| [**RustDesk**](https://github.com/rustdesk/rustdesk) (Rust) | `VideoFrameController` ack-gated production + `VideoQoS` RTT-driven adaptive controller; multi-client shared bitrate. Validates ack-gating independently of Guacamole. |
| [**Xpra**](https://github.com/Xpra-org/xpra) | Damage rects + **scroll pseudo-encoding** (motion vectors) + content-type escalation. The scroll-vector idea is the standout remote bandwidth win. |
| [**rerun.io**](https://github.com/rerun-io/rerun) (Rust) | Architectural twin: authoritative in-RAM store + Arrow delta stream + wgpu render (`re_renderer` does WebGPU-when-available, auto WebGL2 fallback — the dual-path abstraction to copy). |
| [**Figma** rendering + kiwi](https://madebyevan.com/figma/) | Retained-mode GPU geometry (don't re-upload per frame); tile/dirty-region repaint; **schema-stripped binary deltas over a persistent socket** = our wire-format template. |

### Tier ★★ — targeted lessons
| Repo | Lesson |
|---|---|
| [**Alacritty**](https://github.com/alacritty/alacritty) | Ring-buffer grid + display offset + damage (buffer-age PR #5863). We already depend on it; reference for the JS-side scrollback ring-buffer mirror. |
| [**Kitty**](https://github.com/kovidgoyal/kitty/blob/master/docs/performance.rst) | Separate I/O thread from render (our daemon already is this); VRAM glyph cache; SIMD parse. |
| [**WezTerm**](https://github.com/wezterm/wezterm) | `GlyphCache` multi-level atlas via `guillotiere`; the wgpu path for C. |
| [**moonlight-common-c**](https://github.com/moonlight-stream/moonlight-common-c) | Reed-Solomon FEC instead of retransmit — resilience layer for a lossy K2 Connect tunnel. |
| [**webrtc-rs**](https://github.com/webrtc-rs/webrtc) (Rust, sans-IO) | Concrete Rust unreliable-datachannel building block if K2 Connect wants UDP + FEC instead of WS-over-TCP. |
| [**AG Grid**](https://blog.ag-grid.com/optimising-html5-canvas-rendering-best-practices-and-techniques/) / [**Glide Data Grid**](https://github.com/glideapps/glide-data-grid) | High-frequency grid discipline: coalesce ticks to the frame, dirty-cell-only repaint, canvas beats virtualized DOM at high cell counts. A terminal IS a styled cell grid. |
| [**noVNC / RFB**](https://datatracker.ietf.org/doc/html/rfc6143) | CopyRect = canonical scroll primitive; client-pull `FramebufferUpdateRequest` as an alt flow-control model. |
| [**RDP / FreeRDP**](https://deepwiki.com/FreeRDP/FreeRDP/6-graphics-and-display) | Glyph cache (send a styled glyph once, reference by ID) — big win for repeated chars over a tunnel. |
| [**tldraw / Signia**](https://signia.tldraw.dev/docs/incremental) | Incremental reactive signals + viewport culling — single reactor over the delta stream, cull off-screen scrollback. |
| [**tauri-plugin-steam-overlay**](https://github.com/qwook/tauri-plugin-steam-overlay) | The working macOS proof for Path C compositing (transparent click-through child window over wgpu child window). |
| [**FabianLars/tauri-v2-wgpu**](https://github.com/FabianLars/tauri-v2-wgpu) | Official-ish single-window wgpu-from-webview-window example (note: no clean macOS redraw-request hook yet). |

### Tier ★ — context / baselines
KasmVNC (per-region hot/cold quality), neko (WebRTC fan-out validates
one-session-many-windows), selkies (single-port WS+TURN topology), ParaView/VTK
(image-vs-geometry delivery decision framing), Servo WebRender (everything-is-a-quad
atlas), Textual (fractional-line scroll math), Ghostty #2355 (fractional-line
renderer design), ttyd/gotty/wetty (PTY→WS→client wiring baselines),
Graphite (Rust/wgpu editor blocked on the same Linux compositing issue — watch it).

---

## 8. Open questions — what we likely have to figure out ourselves

This setup is new enough that several things have **no off-the-shelf
answer**; these are the genuine R&D items:

1. **One grid → two regimes** (§6): the regime-switching cadence/coalescing
   controller over a shared frame schema. No prior art ships this for a
   terminal.
2. **WebGL2 painter fed by *partial* deltas** (not full frames): xterm's
   painter rebuilds from its own emulator; ours must apply daemon
   CellRun/copy-ref deltas into the instance buffer incrementally. The
   delta→instance-buffer mapping is ours to design.
3. **Scroll/copy-ref protocol** for a *styled cell grid* (vs VNC's pixel
   CopyRect / Broadway's node reposition) — needs our own encoding.
4. **Selection + IME over a GPU canvas** if we ever go Path C — likely
   forces the B′+C hybrid (transparent webview overlay for text); unproven.
5. **macOS WKWebView redraw/vsync ownership** for Path C — Tauri doesn't
   expose it cleanly yet (FabianLars TODO); we'd drive our own loop.
6. **Reconnect resync without a spike** — intra-refresh-style staged
   full-grid snapshot; interacts with the 0.39.5 boot-status handshake and
   0.39.8/0.39.9 reconnect work already shipped.

---

## 9. Suggested phasing (for when this graduates from R&D to build)

- **Phase 0 — Option A quick win** (days): rAF scroll, transform-based
  viewport, fix memo churn, JS scrollback ring buffer, ~8ms daemon
  coalescing. Ships smoother scroll on the current DOM renderer; de-risks
  the delta-cadence work.
- **Phase 1 — Flow control + binary frame** (days–1 week): Guacamole-style
  `sync`/ack loop propagated to PTF pause/resume; JSON → binary frame
  (postcard / DataView). Benefits *both* regimes immediately; prerequisite
  for everything else.
- **Phase 2 — B′ WebGL2 painter** (weeks): xterm-addon-webgl-modeled
  instanced painter w/ Zed alpha-atlas, deltas written into the instance
  buffer. The local-smoothness ceiling without breaking daemon-authority.
- **Phase 3 — K2 Connect hardening** (weeks): scroll/copy-ref encoding,
  RTT-driven coalescing, WebTransport path w/ WS fallback, optional FEC/
  unreliable lane.
- **Phase 4 (escape hatch) — native wgpu (C)**: only if B′ hits a ceiling;
  prototype against steam-overlay child-window pattern; accept the
  IME/selection hybrid.

---

## 10. Local research clones

The highest-value repos are checked out **shallow** as read-only study
material at the sibling path
`/Users/z3thon/DevProjects/Alakazam Labs/terminal-research-repos/`
(outside the K2SO git repo, so they never pollute it). A per-repo study
map — *which files/subsystems to read in each, mapped to our design
decisions* — lives in that directory's `INDEX.md`.

- **Zed** is the #1 reference and is NOT re-cloned — it's already at the
  sibling path `../Zed`. Study `crates/terminal/src/terminal.rs` +
  `crates/terminal_view/src/terminal_element.rs`.
- Cloned (Tier ★★★/★★, source-worth-reading): `xterm.js`, `glyphon`,
  `cosmic-text`, `wezterm`, `alacritty`, `guacamole-server`, `rustdesk`,
  `xpra`, `noVNC`, `moonlight-common-c`, `webrtc-rs`, `wtransport`,
  `tauri-plugin-steam-overlay`, `tauri-v2-wgpu`, `rerun`, `glide-data-grid`.
- Doc-only / clone-on-demand (closed-source or too large): GTK Broadway,
  Figma, FreeRDP, wgpu, Servo WebRender, Bevy, VS Code, ParaView, KasmVNC,
  neko, selkies, ttyd, signia/tldraw. See §7 Tier ★ + `INDEX.md`.

Re-run/refresh: `/tmp/clone_research.sh` is idempotent (skips existing).
The repo list also lives in that script for re-cloning on another machine.

## 11. One-line conclusion

**Smoothness is a pipeline problem, not a draw-speed problem.** Keep the
grid authoritative in Rust; render it locally with a WebGL2 instanced-quad
painter (B′) fed by deltas; harden the same delta stream for remote with
ack-gated backpressure + scroll/copy refs + WebTransport — and refuse the
cloud-gaming reflex to turn text into video. The current low-bandwidth
approach isn't a liability to replace; it's the **foundation K2 Connect
should be built on**, with a GPU painter bolted onto the local end.
