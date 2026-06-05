//! K2 Connect "Clone to" — the pure bundle engine.
//!
//! An agent's full state lives in **three** filesystem locations, and a
//! one-click migration to a remote host has to gather all three, scrub
//! the secrets, drop the bulk, and package the result so the remote can
//! reconstruct a resumable agent at recomputed paths. This module is the
//! shared CORE that does the gathering + scrubbing (this commit) +
//! packaging (follow-up commit). It has NO HTTP, NO Tauri, NO daemon — a
//! pure, unit-testable library consumed by both the high-bar "Clone to"
//! push (P2) and the save-locally README fallback (P3). See PRD
//! `k2-connect-clone-to.md`.
//!
//! ## The three locations
//! For a project at `PROJECT` with
//! `SLUG = chat_history::claude_project_hash(PROJECT)`:
//! 1. **Workspace dir** — the `PROJECT` tree, minus excludes + scrubbed
//!    secrets.
//! 2. **Durable memory** — the entire `<home>/.claude/projects/<SLUG>/
//!    memory/` directory.
//! 3. **Live session(s)** — `<session-id>.jsonl` directly under
//!    `<home>/.claude/projects/<SLUG>/`. Default: the newest-mtime one;
//!    opt-in: all of them (+ `<SLUG>-<branch>/` worktree variants).

mod inventory;
mod scrub;

pub use inventory::inventory;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Which of the three state locations a bundled file belongs to. Drives
/// where the remote unpack route places the file: `Workspace` files land
/// at `DEST_PATH`; `Memory` + `Session` land under the remote-recomputed
/// `<home>/.claude/projects/<remote-slug>/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DestinationClass {
    /// A file from the PROJECT workspace tree.
    Workspace,
    /// A file from `~/.claude/projects/<slug>/memory/`.
    Memory,
    /// A `<session-id>.jsonl` under `~/.claude/projects/<slug>/` (or a
    /// `<slug>-<branch>/` worktree variant).
    Session,
}

/// Options controlling what `inventory()` gathers.
#[derive(Debug, Clone)]
pub struct CloneOptions {
    /// Include EVERY `<session-id>.jsonl` under the slug dir (and worktree
    /// `<slug>-<branch>/` variants), not just the newest-mtime live one.
    /// Default `false` — the live session only.
    pub include_all_history: bool,

    /// Carry secrets over the (encrypted) link instead of scrubbing them.
    /// Default `false` — secrets are excluded from the bundle and recorded
    /// in the re-supply list. `~/.claude/.credentials.json` is excluded
    /// regardless (it is never even enumerated).
    pub carry_secrets: bool,

    /// Home directory override for hermetic resolution of the
    /// `~/.claude/projects/<slug>/` memory + session locations. `None`
    /// uses `dirs::home_dir()`. Tests set this to a temp dir so the
    /// real home is never touched.
    pub home_override: Option<PathBuf>,
}

impl Default for CloneOptions {
    fn default() -> Self {
        Self {
            include_all_history: false,
            carry_secrets: false,
            home_override: None,
        }
    }
}

/// One file selected for the bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryEntry {
    /// Absolute path on the source machine. Not serialized into the
    /// manifest (the bundle is portable).
    #[serde(skip)]
    pub abs_path: PathBuf,
    /// Path relative to the location root, used as the destination key
    /// inside the per-class subdir. For `Workspace` this is relative to
    /// `PROJECT`; for `Memory` relative to `.../<slug>/memory/`; for
    /// `Session` relative to `.../projects/` (so worktree variants keep
    /// their `<slug>-<branch>/<id>.jsonl` shape).
    pub rel_path: String,
    /// Destination class.
    pub class: DestinationClass,
}

/// The structured result of an `inventory()` pass.
#[derive(Debug, Clone)]
pub struct CloneInventory {
    /// Absolute, resolved PROJECT path.
    pub project_path: String,
    /// `claude_project_hash(project_path)` — the SOURCE slug. The remote
    /// recomputes its own slug from the dest path; this is never copied.
    pub slug: String,
    /// Every file selected for the bundle, across all three classes.
    pub entries: Vec<InventoryEntry>,
    /// Relative paths of files scrubbed as secrets (empty when
    /// `carry_secrets` is set). Surfaced verbatim in the re-supply report.
    pub scrubbed_secrets: Vec<String>,
}
