//! Session primitive — the atom of the 0.34.0 Session Stream.
//!
//! This is primitive A from the PRD at
//! `.k2so/prds/session-stream-and-awareness-bus.md`. A session here
//! is a typed event stream owned by the daemon with many producers
//! and many consumers — the unit that lets multiple devices view the
//! same session at their own width without fighting over a shared
//! grid. The harness-neutral `Frame` and width-free `Line` types
//! are the vocabulary every consumer speaks.
//!
//! Phase 1 (this crate's scope) defines the types. Runtime plumbing
//! (broadcast channels, replay ring, archive writer) lands in Phase 2.

pub mod entry;
pub mod frame;
pub mod line;
pub mod registry;
pub mod types;

pub use entry::{SessionEntry, BROADCAST_CAP, REPLAY_CAP};
pub use frame::{CursorOp, EraseMode, Frame, SemanticKind, Style};
pub use line::{Line, SeqnoGen, SequenceNo};
pub use types::{HarnessKind, Session, SessionId};
