# Clone-to Completeness — slug-encoder alignment + ALL-provider session migration

**Status:** DRAFT (proposed for 0.39.44 / 0.40) — pending Explore validation pass
**Author:** pod-leader (with Rosson)
**Related:** GH#25 (clone receives zero sessions), GH#23 (cwd not rewritten —
0.39.40), GH#21/#671 (bundle all Claude sessions — 0.39.38), the Issue-B
"path-divergence" (daemon-multi-client-arbitration PRD). Clone module:
`crates/k2so-core/src/clone/{mod,bundle,inventory,unpack,repair,scrub,settings}.rs`.

Two goals: (1) fix the **slug-encoder mismatch** that makes Clone-to (and
`/resume`, and the pinned-chat dropdown) silently miss Claude sessions; (2)
extend Clone-to to migrate **every** provider's chat history (Claude, Cursor,
Gemini, Pi, Codex) — not just Claude.

---

## Part A — `claude_project_hash` must match Claude Code's encoder (the #25 root)

### Problem (empirically confirmed)
`crates/k2so-core/src/chat_history.rs::claude_project_hash` is:
```rust
project_path.replace("/.", "/-").replace('/', "-").replace(' ', "-")
```
A live probe (`claude -p` in `/tmp/k2so_slug_probe_a_b/has_under_score.dotted`)
showed Claude Code writes the slug:
```
-private-tmp-k2so-slug-probe-a-b-has-under-score-dotted
```
K2SO would compute `-tmp-k2so_slug_probe_a_b-has_under_score.dotted`. **Two
divergences:**
1. **Characters:** Claude maps every non-`[a-zA-Z0-9]` char → `-` (so `_`, `.`,
   ` `, `/` all → `-`). K2SO **preserves `_` and mid-component `.`**. → For any
   workspace path with `_` or a dotted component, K2SO and Claude use DIFFERENT
   slug dirs and never see each other's sessions.
2. **Realpath:** Claude **canonicalizes symlinks** (`/tmp`→`/private/tmp`,
   `/var`→`/private/var`); K2SO uses the literal path.

### Impact (this one bug, many symptoms)
- **#25 Finding 1:** the bundler enumerates `~/.claude/projects/<K2SO-slug>/`
  which doesn't exist (Claude wrote the hyphen slug) → **0 sessions bundled**;
  and unpack WRITES to the K2SO-slug dir while Claude READS the hyphen dir.
- **#25 Finding 2:** exactly this (the ticket had the direction reversed —
  K2SO preserves `_`, Claude collapses it).
- **#23** (`/resume` empty on dest) and the **Issue-B pinned-dropdown
  "path-divergence"** (`exists(B)=false` for `_`/symlink paths).

### Fix
Rewrite `claude_project_hash` to mirror Claude exactly:
```rust
pub fn claude_project_hash(project_path: &str) -> String {
    let canonical = std::fs::canonicalize(project_path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| project_path.to_string());
    canonical.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect()
}
```
- **Confirm in Explore** Claude's exact rule on a couple more chars (consecutive
  separators / leading-dot hidden dirs → does Claude emit `--`? our probe + the
  existing `/.`→`/-` test suggest yes; verify so the new mapping reproduces it).
- `claude_project_hash` is used EVERYWHERE (clone/unpack, resume, exists-check,
  `newest_claude_session_on_disk`, repair) — aligning it fixes all of them at
  once. It only CHANGES output for `_`/dotted/symlink paths (the broken cases);
  plain paths are unchanged. Existing tests with no `_` stay green; update any
  that assumed the old rule.

### Self-heal migration (mis-slugged unpack dirs)
Earlier K2SO clones unpacked sessions into the WRONG (underscore/literal) slug
dir. A boot-time, idempotent sweep (extend `clone/repair.rs`): for each
registered project, if the OLD-encoder slug dir exists AND the NEW-encoder slug
dir does not, move/merge it to the correct dir (then the existing #23 cwd-rewrite
applies). Non-fatal, logged.

---

## Part B — bundle Claude worktree sub-slugs

Per #25 Finding 1: a user whose chat work happened in `<workspace>/.worktrees/
<branch>/…` (or `.scout-worktrees/…`) has sessions under SIBLING slug dirs
(`<slug>-<branch>` / a separately-hashed worktree path), which the
workspace-root bundler never sees. **Include** every `~/.claude/projects/` dir
whose decoded cwd is the workspace OR a path under it (root + `<slug>-*`
worktree siblings — `claude_session_file_exists` already scans that set; reuse
the dir-match logic). Unpack each to the dest's recomputed slug.

---

## Part C — migrate ALL providers' history, not just Claude

Clone-to today bundles only Claude (`DestinationClass::Session`). Generalize so a
clone carries **every provider's** sessions for the workspace: **Claude, Cursor,
Gemini, Pi, Codex**. K2SO already knows how to FIND each provider's sessions for
a workspace (the `detect_*` / `parse_*` helpers in `chat_history.rs`); the clone
path must reuse that knowledge to **bundle → unpack → rewrite embedded paths**.

### Provider storage (from chat_history.rs — confirm exact shapes in Explore)
| Provider | On disk | Workspace key | Embedded cwd to rewrite? |
|---|---|---|---|
| **Claude** | `~/.claude/projects/<slug>/*.jsonl` (+ worktree siblings) | `claude_project_hash` | yes — `cwd` in `.jsonl` (#23 byte-replace) |
| **Cursor** | `~/.cursor/chats/<hash>/*/store.db` (SQLite) | `cursor_project_hash` | TBD — path inside `store.db`; SQLite, not text |
| **Gemini** | `~/.gemini/tmp/<id>/…` + `~/.gemini/projects.json` (slug→cwd map) | gemini slug map | yes — `projects.json` mapping + any embedded cwd |
| **Pi** | `~/.pi/agent/sessions/…` | per-session (cwd inside?) | TBD |
| **Codex** | `~/.codex/sessions/…` (+ `history.jsonl`) | per-session (cwd inside?) | TBD |

### Design — a per-provider clone adapter
Introduce a small trait/enum so each provider plugs into the same
bundle/unpack pipeline:
```
trait SessionProvider {
    fn name(&self) -> &str;                       // "claude" | "cursor" | …
    fn locate(&self, home, workspace_path) -> Vec<SessionArtifact>;  // files/dirs for this workspace
    fn dest_path(&self, home, dest_workspace_path, artifact) -> PathBuf; // where it lands on the dest
    fn rewrite_embedded_paths(&self, bytes, src_workspace, dest_workspace); // per-format cwd rewrite
}
```
- **Bundle:** for each provider, `locate` the workspace's artifacts; add to the
  tarball under a provider-namespaced class (`sessions/claude/…`,
  `sessions/cursor/…`, …) so unpack routes each to the right home subtree.
- **Unpack:** for each artifact, compute `dest_path` on the destination
  (re-encoded slug / re-keyed), write it, and `rewrite_embedded_paths`
  (SOURCE workspace path → DEST workspace path) so the migrated session resolves
  on the destination — the #23 fix generalized per format (byte-replace for
  jsonl; SQL `UPDATE`/blob-rewrite or path-table fix for Cursor's SQLite;
  `projects.json` remap for Gemini; per-format for Pi/Codex).
- **Manifest:** extend `CloneManifest` to record per-provider counts +
  `source_workspace_path` (already present) so the dest can rewrite + report.
- **Back-compat:** old bundles (Claude-only `sessions/<rel>`) still unpack;
  the new layout is additive (`sessions/<provider>/<rel>`).

### Open per-provider questions (Explore)
- **Cursor:** `store.db` is SQLite — where is the workspace path stored (a
  column? a JSON blob?), and can we rewrite it on the dest, or do we accept it
  may not perfectly re-root? Is `cursor_project_hash` deterministic from the
  cwd (so the dest slug is recomputable)?
- **Gemini:** how does `projects.json` map slug→cwd, and is the session content
  under `tmp/<id>/` keyed by that slug? On unpack, add a dest mapping + copy the
  tmp dir.
- **Pi / Codex:** how is a session associated with a workspace cwd (a field in
  each session file? a per-session dir name?)? Where's the cwd embedded?
- For each: is there an embedded absolute path that breaks resume on the dest
  (the #23 class of bug), and what's the rewrite mechanism per format?

---

## Part D — observability (Finding 1 recommendation)
- **Send:** log per-provider session counts being bundled; if total sessions
  == 0, a prominent warning (and surface "0 sessions found to migrate" in the
  Clone-to dialog so the user knows BEFORE sending).
- **Receive:** log the unpack summary (`N workspace files, per-provider session
  counts`) to `daemon.stderr.log` — #25 noted the receive produced **no** log
  line at all. Surface "migrated N conversations across M tools" in the receive
  UI.

---

## Phasing
1. **Phase 1 (0.39.44, small + high-value):** Part A encoder fix + self-heal +
   Part D send/receive logging. This alone fixes #25 for Claude (bundling +
   unpack + resume) AND the Issue-B dropdown path-divergence. Ship fast.
2. **Phase 2:** Part B Claude worktree sub-slugs.
3. **Phase 3 (the big feature):** Part C multi-provider bundling, provider by
   provider (Claude already covered; add Cursor, Gemini, Pi, Codex) behind the
   adapter, each with its own rewrite + a round-trip test.

## Tests
- Encoder: `claude_project_hash` matches Claude's real output for `_`, `.`,
  spaces, symlinked roots (golden cases from the probe). Existing call sites
  resolve real Claude dirs.
- Clone round-trip per provider: bundle on a source layout → unpack to a dest
  path → the provider's `detect_*`/`parse_*` finds the migrated sessions at the
  dest, and resume works (embedded path rewritten). Zero-session bundle logs the
  warning.

## Open questions for Explore
1. Confirm Claude's exact encoder rule (consecutive separators, leading dots).
2. Map the current `bundle.rs`/`inventory.rs`/`unpack.rs` flow + where
   `claude_project_hash` + the `DestinationClass::Session` enumeration happen.
3. Per-provider storage + workspace-key + embedded-cwd shape (the §C table).
4. Cursor SQLite re-rooting feasibility (the hardest provider).
5. Whether `include_all_history` (the 0.39.38 toggle) is correctly threaded and
   defaulting on (Finding 1 cause #2).
