# K2SO 0.39.4 — What's New popup: full minor-track context

Patch release. Tiny but important UX fix to the "What's new"
popup itself.

## What this fixes

The "What's new" popup uses the daemon's `whats_new_check` to ask
"is there a section the user hasn't seen?" If yes, the popup
opens with the unseen markdown and lets the user paginate with
←/→ buttons.

Pre-0.39.4 the daemon shipped a slice of `(last_seen_version,
current_version]` — only entries newer than what the user last
dismissed. That worked fine when the user updated from a stable
release to the next minor (e.g. 0.38.13 → 0.39.0 showed every
0.39.x section). It failed users who joined a minor track
mid-stream:

- User installed 0.39.2 → marked seen.
- Auto-update lifted them to 0.39.3.
- Popup slice = `(0.39.2, 0.39.3]` = just the 0.39.3 entry.
- They never saw the **0.39.0** entry — which is where the
  foundational agent-pinning changes were explained, and where
  the one-time migration that re-arranged their sidebar
  documented itself.

User-visible symptom: "Why did my Agents section disappear? Why
is my pinned list suddenly full of workspaces?" The answer was
sitting in the 0.39.0 entry, hidden from anyone who joined the
track late.

## The fix

`check_for_user`'s **decision to fire** still uses
`(last_seen, current]` — the popup only auto-opens when there's
genuinely new content.

But when it fires, the **content payload** now spans the full
current MAJOR.MINOR track up through `current_version` (e.g.
on 0.39.4 it ships 0.39.0 + 0.39.1 + 0.39.2 + 0.39.3 + 0.39.4).
The modal's existing ←/→ pagination already handles per-version
pages, so the user can walk back to the start of their current
minor track and read why their sidebar looks different.

### Code changes

- **`crates/k2so-core/src/whats_new.rs`**:
  - New `slice_minor_track(sections, current)` returns every
    entry whose MAJOR.MINOR matches `current`'s and whose
    version `<= current`.
  - `check_for_user` keeps `has_new` driven by `slice_unseen`
    (auto-fire semantics are unchanged), but populates
    `content` with `slice_minor_track` when firing.
  - New private `minor_track(version)` helper extracts the
    `MAJOR.MINOR` prefix, defensive on malformed input.
  - 5 new inline tests cover the helper's behaviour (track
    extraction, current-bound, track-opener, empty track,
    malformed input).

- **`WHATS_NEW.md`**: the 0.39.4 entry actively prompts the
  reader to hit ← and walk back to the 0.39.0 entry, with the
  context for *why* that matters (the sidebar / pin
  rearrangement is the change a mid-track joiner is most likely
  to be confused by).

### Why this matters beyond 0.39.x

The "full minor-track on fire" semantic is the right default for
every future minor:

- Landing on `0.40.3` (after 0.40.0 / 0.40.1 / 0.40.2) shows the
  full 0.40.x catalogue, regardless of which 0.40.x patch you
  came from.
- Patch-level fixes that depend on a feature shipped in
  `X.Y.0` always carry that feature's intro entry along with
  them, so users never get a context-free patch note.

It also means hot-fix sequences (like the four 0.39.x patches
in May 2026) form a coherent story the user can read end-to-end.

## Tested

- `cargo test -p k2so-core --lib whats_new` — all 24 tests pass,
  including the 5 new minor-track tests covering:
  - Includes every in-track entry `<= current`.
  - Excludes older minors (no 0.38.x leakage).
  - Caps at `current` so a stale-on-disk WHATS_NEW.md with
    future patches doesn't leak `0.39.4`'s entry into a daemon
    still pinned at `0.39.1`.
  - At the track opener (`0.39.0`) returns just that entry.
  - Empty when no sections in the asked track (e.g. `0.40.0`
    against a file that only has `0.39.x`).

## Upgrade notes

- Any 0.39.x → 0.39.4: clean update. First launch fires the
  popup on the 0.39.4 entry; arrow left to read back through
  0.39.3 → 0.39.2 → 0.39.1 → 0.39.0.
- Users on 0.38.x → 0.39.4: full migration sequence from 0.39.0
  + 0.39.1 fires on first boot (one-time auto-pin + corrective
  unpin); popup ships every 0.39.x entry up to 0.39.4.

## What else shipped in this release

Nothing else. See `release-notes-0.39.0.md` through
`release-notes-0.39.3.md` for prior content.
