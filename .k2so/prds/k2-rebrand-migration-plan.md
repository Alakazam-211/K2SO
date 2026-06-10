# K2 Rebrand & Repo Migration Plan — K2SO → K2 (0.39.48 bridge → 0.40.0)

**Status:** PLAN for Rosson review (2026-06-10). Repo `Alakazam-211/K2` is
LIVE (public) with FSL-1.1-Apache-2.0 + draft Hosting Grant + draft
Trademark policy + pre-0.40 README.
**Brand:** "K2" (uppercase) publicly; `k2` lowercase for CLI/machine-facing;
"K2 by Alakazam Labs" composite brand (per the §2(d) house-mark decision).
**Related:** `k2-licensing-strategy.md`, GH #613 (rename), memory
`project_k2_product_taxonomy`.

---

## 0. The two insights that shape the whole plan

### A. The updater redirect lives in a MANIFEST, not in a client release
Installed apps poll a URL **baked into their tauri.conf**:
`github.com/Alakazam-211/K2SO/releases/latest/download/latest.json`.
The manifest's platform `url` field is **absolute** — it can point at an
asset on ANY repo. So the cross-repo jump is: publish a final release on
the OLD repo whose `latest.json` says `version: 0.40.0` with
`url: …/Alakazam-211/K2/releases/download/v0.40.0/K2.app.tar.gz`, signed
with the SAME updater key. Old apps follow it automatically. 0.39.48 is
therefore NOT strictly required for the redirect — but it IS required for
other reasons (§3).

### B. The old repo becomes a PUBLIC SHELL — binaries + bridge, no source
(Decision updated 2026-06-10 after Rosson: "I don't want the code under
MIT anymore.") The baked-in manifest URL forces the repo to stay PUBLIC in
some form — private = stragglers stranded + dead DMG links. But it need
not contain source. **The shell maneuver:**
1. publish the final bridge release;
2. create an orphan stub commit (pointer README only);
3. FORCE-MOVE every release tag onto the stub + force-push main to it,
   delete other branches → releases keep serving assets (bound to tag
   NAMES) and auto source-zips regenerate FROM THE STUB; the old history
   becomes unreachable (no forks pin it) and falls to GitHub GC (support
   ticket hastens);
4. retro-edit every old release's NOTES with a "K2SO is now K2 →" banner
   (gh release edit --notes);
5. archive the shell read-only.

**HARD GATE:** deleting a tag drafts its release (bridge-fatal); MOVING a
tag is the safe variant — verify the full sequence on a SACRIFICIAL repo
(tag move → release still published → latest/download alias serves →
source-zip = stub) before running it on K2SO.

Legal honesty, stated once: this stops Alakazam DISTRIBUTING the MIT
source; it cannot revoke MIT on copies already out there (560 unique
cloners in the last 14 days alone; third-party archives like Software
Heritage mirror public repos). The forward-looking protection is FSL +
trademark + the never-published control plane. Optional later stage: once
update telemetry shows the fleet has crossed, the shell can be privated
entirely (strands stragglers — decide with data, months out).

## 1. Rename surface inventory (compat policy per class)

| Class | Today | 0.40.0 | Compat |
|---|---|---|---|
| Public brand / UI strings | K2SO | **K2** ("K2 by Alakazam Labs" in About/marketing) | none needed |
| CLI binary | `k2so` | **`k2`** | `k2so` shim delegates + prints one-line deprecation warning (Rosson-specified) |
| App bundle | K2SO.app / com.alakazamlabs.k2so | K2.app / **dev.k2.app** (pending Q1) | updater rename rig decides mechanics (§2) |
| Daemon binary | k2so-daemon | k2-daemon | old name symlink for launchd transition window |
| Crates | k2so-core, k2so-daemon | k2-core, k2-daemon | internal — rename freely |
| launchd labels | com.k2so.k2so-daemon, com.k2so.agent-heartbeat, com.k2so.claude-auth-refresh | dev.k2.daemon, dev.k2.heartbeat, dev.k2.claude-auth (pending Q1) | 0.40.0 first-boot migration: bootout old, bootstrap new |
| Home dir | ~/.k2so (db, ports, tokens, logs, frpc) | **~/.k2** | first-boot move + `~/.k2so` → symlink to `~/.k2` for external tooling |
| Per-workspace dir | `.k2so/` (in USERS' repos: agents, inbox, prds, skills) | **keep `.k2so/` for 0.40** (Q2) | touching users' repos is the riskiest rename; defer to a later dual-read `.k2/` phase |
| Env vars | K2SO_PROJECT_PATH, K2SO_PANE_ID, … | K2_* emitted | K2SO_* still READ (dual-read) for ≥2 minor versions |
| TERM_PROGRAM / banners / pane ids | K2SO / k2so-daemon / alacritty-v2-* | K2 / k2-daemon | renderer + heuristics updated in lockstep |
| HTTP surface | /cli/* routes, daemon.port files | unchanged | wire compat is sacred (remote clients on older versions) |
| npm / package names | (none published; taxonomy says @alakazamlabs/k2) | claim **@alakazamlabs/k2** | Q4 |
| Skill files (SKILL.md regen, AGENT.md templates) | `k2so …` commands | `k2 …` | regen on 0.40 first boot; old text still works via shim |
| Keychain (K2 Connect login) | held for 0.40 per memory | dev.k2.* | migration copies/re-mints tokens |

## 2. Phase R1 — the decisive experiment (BEFORE anything ships)

**Updater rename rig:** install current K2SO.app in a VM/sacrificial user,
feed it a signed update bundle whose inner app is `K2.app` with a NEW
bundle identifier. Determine:
- does tauri-plugin-updater install it over /Applications/K2SO.app? as
  K2.app? leave both? fail signature/identifier checks?
- do macOS permission grants (notifications etc.) survive?

Outcomes → 0.39.48 scope:
- **Clean rename:** 0.39.48 is cosmetic (announcement + `k2` CLI teaser).
- **Updater can't rename:** choose between (a) 0.39.48 ships a custom
  install hook that performs app-swap (download new, bless, remove old,
  relaunch), or (b) 0.40.0 keeps `com.alakazamlabs.k2so` identifier +
  K2SO.app bundle name with full K2 *branding inside*, and the file-level
  bundle rename happens via fresh DMG installs only (auto-updated users
  rename at 0.41 or never — display name is what users see anyway).
  Option (b) is the lowest-risk fallback and is fully acceptable.

## 3. Phase plan

**R0 — done 2026-06-10:** `Alakazam-211/K2` public with LICENSE.md (canonical
FSL text + Alakazam Labs notice), COMMERCIAL_HOSTING_GRANT.md (DRAFT),
TRADEMARKS.md (DRAFT), README. Open decisions Q1–Q5 below.

**R1 — updater rename rig** (§2). Nothing else proceeds to ship before
this answers.

**R2 — the rename work** (current repo, feature branch, big but mechanical):
crates/binaries/CLI/env/banners/UI per §1 table; `k2` CLI + `k2so` shim;
~/.k2so→~/.k2 migration module + launchd label migration (runs at 0.40.0
first boot, idempotent, logged); release.sh parameterized for new repo +
asset names; tauri.conf: productName K2, identifier per Q1, updater
endpoint → `Alakazam-211/K2` manifest. Full test suite + a migration
integration test (fake ~/.k2so fixture → boot → assert moved+symlinked).

**R3 — 0.39.48 bridge release (old repo, last K2SO-branded build):**
- In-app rebrand announcement (WHATS_NEW): name change, new home, and the
  LICENSE CHANGE disclosure (0.40+ is Fair Source/FSL; 0.39.x stays MIT) —
  users crossing an auto-update license boundary deserve explicit notice.
- Ship the `k2` CLI alias early (with `k2so` still primary) as a teaser.
- Any updater shim R1 demands.

**R4 — code lands in the new repo:**
- **FRESH-HISTORY import** (corrected 2026-06-10): the public K2 repo
  starts from a single squashed commit ("Imported from K2SO at 0.39.x")
  under FSL. Pushing full history would RE-PUBLISH the entire MIT-era
  source (any old commit is checkout-able and MIT-licensed at that
  commit) — defeating the shell maneuver. Full history is preserved in a
  PRIVATE mirror (`Alakazam-211/K2-history` or local) for blame and
  archaeology; this also disposes of the PRD-privacy question entirely
  (no history → no PRDs ever public on the new repo).
- Internal-docs policy (Q5): public repo `.gitignore`s `.k2so/` agent
  dirs + PRDs; PRDs migrate to the private mirror; only user-facing
  docs/ ship publicly.
- CONTRIBUTING.md with DCO sign-off (layer 5 of licensing strategy).
- **Lawyer-pass gate** on LICENSE/GRANT/TRADEMARKS before the first
  release is published here.
- 0.40.0 released on the NEW repo via updated release.sh.

**R5 — the cutover (one afternoon, ordered):**
1. 0.40.0 live + verified on new repo (fresh-install DMG path tested).
2. Publish on the OLD repo the final bridge release: `latest.json`
   (version 0.40.0, url → new-repo asset, same signing key) +
   `daemon-latest.json` equivalents for the P3 headless self-update.
3. Verify an existing 0.39.x install auto-updates across repos cleanly
   (and a headless daemon self-updates).
4. Old repo: run the SHELL MANEUVER (§0.B — stub commit, tag force-move,
   branch deletion, retro-edited release notes), then **Archive (public,
   read-only)**. PRD scrub is subsumed — the stub removes the whole tree.
5. k2.dev: copy + download links + Fair Source page + hosting-grant page.

**R6 — aftermath:** close #613; claim @alakazamlabs/k2 (Q4); update
memories (project_vision "MIT" reference, taxonomy); monitor old-repo
traffic for stragglers; bridge manifest stays frozen forever.

## 4. "Nobody gets stuck" guarantees (the worry, answered)

- Anyone on **0.39.x**: their app polls the old manifest forever; the
  bridge manifest hands them 0.40.0 from the new repo whenever they
  update — months or years later. Works because old repo = archived
  PUBLIC.
- Anyone wanting an **old version**: every historical release/DMG stays
  downloadable on the archived repo.
- **Headless daemons** (P3 self-update): same bridge treatment via
  daemon-latest.json.
- Anyone offline through the whole era: fresh DMG from k2.dev/new repo;
  0.40.0's first-boot migration handles their ~/.k2so data regardless of
  which version it came from.

## 5. Open questions for Rosson (Q1–Q5)

1. **Internal IDs:** adopt `dev.k2.*` (bundle id `dev.k2.app`, launchd
   `dev.k2.daemon` etc., keychain `dev.k2.*`) per the held 0.40 decision —
   confirm? (Identifier change = macOS treats it as a new app: permission
   prompts re-appear once; R1 rig will quantify.)
2. **Per-workspace `.k2so/` dirs:** keep for 0.40 (recommended — they live
   in users' repos and agents reference them), with dual-read `.k2/`
   arriving later? Or hard-rename now with a `k2 migrate-workspace`
   command?
3. **Copyright entity:** LICENSE notice currently "Alakazam Labs"; code
   signing is "LZTEK, LLC". Lawyer should confirm which entity holds the
   IP (and the © line should match it).
4. **npm:** claim `@alakazamlabs/k2` now? (taxonomy memory expects it)
5. **PRD privacy:** confirmed direction — new repo never carries internal
   PRDs (public repo gitignores agent/PRD dirs; internal docs move to a
   private home). Old-repo exposure is resolved by the shell maneuver.
