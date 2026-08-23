//! Core scrollback types.
//!
//! Vendored from xai-org/grok-build `xai-grok-pager`
//! `src/scrollback/types.rs` + `src/scrollback/state/types.rs`
//! (Apache-2.0). Local adaptation: only the types pi-rs's minimal pager
//! needs (`DisplayMode`, `EntryLayoutInfo`); pager-specific types
//! (Turn/ViewMode/BlockLine/BlockOutput/selection) and the
//! `appearance`/`theme` dependencies are dropped.

/// How a block is currently displayed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum DisplayMode {
    /// Single synthetic header row (`▸ write` / `▸ 3 tool calls`).
    Collapsed,
    /// First N lines plus a "N more" header.
    Truncated,
    #[default]
    Expanded,
}

/// Per-entry layout info, derived by the fold projection
/// (`groups::project_to_layout`). The renderer and interaction layer consume
/// only this — they never re-derive fold shapes.
#[derive(Debug, Clone, Copy, Default)]
pub struct EntryLayoutInfo {
    /// Rendered height at current width.
    pub height: u16,
    /// Gap rows after this entry (0 for dense group members, 1 otherwise).
    pub gap_after: u16,
    /// When non-zero, this entry renders as a group header instead of its
    /// normal block content. For N-more truncation headers it is the number
    /// of hidden lines (drives the plain "N more" text). For verb-group
    /// headers it is the run's MEMBER count — the value drives the
    /// aggregated "N tool calls" label.
    pub group_header_count: u16,
    /// When true, this entry renders as an expanded-group collapse header
    /// (e.g. the `▾ N tool calls` line above the members of a
    /// manually-expanded verb group).
    pub group_collapse_header: bool,
    /// When true, this entry heads a verb-group run: it renders the
    /// aggregated "N tool calls" label instead of its own content
    /// (collapsed state) or marks the collapse header of an expanded verb
    /// group. The run's other claimed entries hide behind it (height 0)
    /// until the group is expanded.
    pub verb_group_header: bool,
}

impl EntryLayoutInfo {
    /// Whether this entry renders as any kind of group header (N-more
    /// truncation, expanded-group collapse, or verb) in place of its own
    /// block content. The single gate shared by every consumer, so no site
    /// re-derives it from the raw fields and silently drops one header
    /// family (the count's meaning differs per family; see
    /// [`Self::group_header_count`]).
    pub fn is_group_header(&self) -> bool {
        self.group_header_count > 0 || self.group_collapse_header
    }
}
