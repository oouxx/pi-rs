//! Scrollback — conversation display with blocks, folds, and scroll.
//!
//! Vendored (file-level) from xai-org/grok-build `xai-grok-pager`
//! `src/scrollback/` (Apache-2.0) — see THIRD-PARTY-NOTICES.md §4 for the
//! upstream revision and the local adaptation list.
//!
//! Philosophy carried over from grok:
//!
//! - **Entry model** — everything displayed (messages, tool calls) is an
//!   entry with a stable id; fold state is keyed by id, never by position.
//! - **Folds are a derived model** — fold shapes (`GroupSpan`s) are
//!   re-scanned from the entries on every structural change; a single
//!   projection pass (`project_to_layout`) writes the per-entry flags the
//!   renderer consumes. Consumers can never observe a fold shape the model
//!   doesn't describe.
//! - **Two fold families** — verb runs (eagerly folded runs of finished
//!   tool calls: the aggregated "N tool calls" header) and truncation
//!   ("N more" headers for over-long content).
//! - **User override wins** — `expanded_groups` / `collapsed_groups`
//!   (keyed by the first entry's id) override the derived default.

pub mod groups;
pub mod types;

pub use groups::{GroupKind, GroupSpan};
pub use types::{DisplayMode, EntryLayoutInfo};
