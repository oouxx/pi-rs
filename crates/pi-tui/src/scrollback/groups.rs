//! Derived group model for the scrollback's view-time folds.
//!
//! Vendored (file-level, adapted) from xai-org/grok-build `xai-grok-pager`
//! `src/scrollback/state/groups.rs` (Apache-2.0) — see
//! THIRD-PARTY-NOTICES.md §4. Local adaptation: the verb-group walk is
//! simplified (no subagent rows, no thinking blocks, no per-verb label
//! aggregation — members are finished tool calls and the aggregated label
//! is "N tool calls"); the `appearance` settings reads are parameters.
//!
//! One scan owns every grouping decision: verb-group runs claim their
//! entries first, then group truncation ("N more") runs over the rest. The
//! scan produces [`GroupSpan`]s — the authoritative description of every
//! fold — and [`project_to_layout`] is the single writer that turns spans
//! into the per-entry [`EntryLayoutInfo`] flags the renderer and navigation
//! consume. Keeping the decision (scan) and the flag writes (projection)
//! in one module means a consumer can never observe a fold shape the model
//! doesn't describe.

use std::collections::HashSet;
use std::ops::Range;

use super::types::{DisplayMode, EntryLayoutInfo};

/// One folded region of the transcript, in entry indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSpan {
    /// Entries the fold walked. For verb runs this ends one past the last
    /// claimed entry. For truncation it is the whole dense run, visible
    /// tail included.
    pub range: Range<usize>,
    /// Which fold produced this span and its count data.
    pub kind: GroupKind,
    /// Whether the user manually expanded this group (keyed by the first
    /// entry's ID in the caller's `expanded_groups`).
    pub expanded: bool,
}

/// The two fold families. Both render a synthetic header row; they differ in
/// when they fold and what the header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    /// Eagerly folded run of finished tool calls — the aggregated
    /// "N tool calls" header. `members` counts the claimed tool calls.
    VerbRun { members: usize },
    /// Budget truncation of an over-long dense run — the "N more" header.
    /// `participants` counts the entries eligible to hide; `hidden` is how
    /// many of them the collapsed state conceals.
    Truncation { participants: usize, hidden: usize },
}

/// A minimal entry view the fold scans read. The caller (pi-tui's app
/// module) derives this from its message/tool model.
#[derive(Debug, Clone)]
pub struct FoldEntry {
    pub id: u64,
    pub display_mode: DisplayMode,
    /// Whether this is a tool-call block (verb-groupable).
    pub is_tool: bool,
    /// Whether this tool call is finished (Done/Failed) — only finished
    /// tools fold into verb runs.
    pub tool_finished: bool,
    /// Total content lines (used by the "N more" count).
    pub total_lines: usize,
}

/// Whether an entry participates in a truncation run: collapsed and
/// foldable.
fn participates_in_truncation(entry: &FoldEntry) -> bool {
    entry.display_mode == DisplayMode::Collapsed
}

/// Binary-search sorted, disjoint spans for the one containing `idx`.
pub fn span_containing(spans: &[GroupSpan], idx: usize) -> Option<&GroupSpan> {
    let pos = spans.partition_point(|s| s.range.end <= idx);
    spans.get(pos).filter(|s| s.range.contains(&idx))
}

/// Scan the transcript for every fold, in the order the folds take
/// precedence: verb runs claim entries first, truncation runs over the
/// rest (claimed entries break truncation runs). Returns spans sorted by
/// start index; spans never overlap.
pub fn scan(
    entries: &[FoldEntry],
    max_visible: usize,
    expanded_groups: &HashSet<u64>,
) -> Vec<GroupSpan> {
    let (mut spans, claimed) = scan_verb_runs(entries, expanded_groups);
    spans.extend(scan_truncations(entries, max_visible, expanded_groups, &claimed));
    // Both scans emit in ascending order over disjoint ranges; interleave.
    spans.sort_unstable_by_key(|s| s.range.start);
    spans
}

/// Find maximal runs of finished collapsed tool calls that fold. Also
/// returns the claimed-entry mask: the truncation scan treats claimed
/// entries as run breakers.
fn scan_verb_runs(
    entries: &[FoldEntry],
    expanded_groups: &HashSet<u64>,
) -> (Vec<GroupSpan>, Vec<bool>) {
    let n = entries.len();
    let mut spans = Vec::new();
    let mut claimed = vec![false; n];

    let mut i = 0;
    while i < n {
        // A verb run is a maximal run of collapsed, finished tool calls.
        // (Local adaptation of grok's `scan_run_forward`: no thinking /
        // subagent members, no transparent slots.)
        if !is_verb_member(&entries[i]) {
            i += 1;
            continue;
        }
        let start = i;
        let mut members = 0;
        let mut j = i;
        while j < n && is_verb_member(&entries[j]) {
            members += 1;
            j += 1;
        }
        // Runs of one stay unfolded (grok `RunScan::folds` gate).
        if members < 2 {
            i = j;
            continue;
        }
        for slot in claimed[start..j].iter_mut() {
            *slot = true;
        }
        let first_id = entries[start].id;
        spans.push(GroupSpan {
            range: start..j,
            kind: GroupKind::VerbRun { members },
            expanded: expanded_groups.contains(&first_id),
        });
        i = j;
    }
    (spans, claimed)
}

fn is_verb_member(e: &FoldEntry) -> bool {
    e.is_tool && e.tool_finished && e.display_mode == DisplayMode::Collapsed
}

/// Find consecutive runs of collapsed entries longer than
/// `max_visible + 1`. Entries claimed by the verb scan break runs.
fn scan_truncations(
    entries: &[FoldEntry],
    max_visible: usize,
    expanded_groups: &HashSet<u64>,
    claimed: &[bool],
) -> Vec<GroupSpan> {
    let mut spans = Vec::new();
    if max_visible == 0 || entries.is_empty() {
        return spans;
    }

    let n = entries.len();
    let mut i = 0;
    while i < n {
        if claimed[i] || !participates_in_truncation(&entries[i]) {
            i += 1;
            continue;
        }

        let group_start = i;
        let mut group_len = 1;
        let mut j = i + 1;
        while j < n {
            if claimed[j] {
                break;
            }
            if participates_in_truncation(&entries[j]) {
                group_len += 1;
            } else {
                break;
            }
            j += 1;
        }
        let group_end = j;

        if group_len <= max_visible + 1 {
            i = group_end;
            continue;
        }

        let first_id = entries[group_start].id;
        spans.push(GroupSpan {
            range: group_start..group_end,
            kind: GroupKind::Truncation {
                participants: group_len,
                hidden: group_len - max_visible,
            },
            expanded: expanded_groups.contains(&first_id),
        });
        i = group_end;
    }
    spans
}

/// The single writer of group heights and header flags. Every layout
/// consequence of a fold happens here, driven only by the spans.
pub fn project_to_layout(
    spans: &[GroupSpan],
    layout: &mut [EntryLayoutInfo],
    max_visible: usize,
) {
    // Reset all fold-derived flags first (grok `apply` does the same before
    // the scan; here the caller passes a clean slice or we reset in place).
    for info in layout.iter_mut() {
        info.group_header_count = 0;
        info.group_collapse_header = false;
        info.verb_group_header = false;
        info.gap_after = 0;
    }

    for span in spans {
        match span.kind {
            GroupKind::VerbRun { members } => {
                project_verb_run(span, members, layout);
            }
            GroupKind::Truncation {
                participants,
                hidden,
            } => {
                project_truncation(span, participants, hidden, layout, max_visible);
            }
        }
    }
}

/// Collapsed: header renders the aggregated label at `height=1`; other
/// claimed entries fold to `height=0`. Expanded: the header slot is an
/// absolute `height=2` — the header line plus the first member's own row —
/// so ALL members, including the first, reveal below it; members keep
/// their normal heights.
fn project_verb_run(span: &GroupSpan, members: usize, layout: &mut [EntryLayoutInfo]) {
    for idx in span.range.clone() {
        let cached = &mut layout[idx];
        if idx == span.range.start {
            cached.verb_group_header = true;
            cached.group_collapse_header = span.expanded;
            cached.group_header_count = members.min(u16::MAX as usize) as u16;
            cached.height = if span.expanded { 2 } else { 1 };
            if idx != span.range.end - 1 {
                cached.gap_after = 0;
            }
        } else if !span.expanded {
            cached.height = 0;
            if idx != span.range.end - 1 {
                cached.gap_after = 0;
            }
        }
    }
}

/// Collapsed: the first participating entry becomes the "N more" header
/// (the header's own lines are hidden), older participants hide, and the
/// last `max_visible` stay untouched. Expanded: every entry keeps its
/// height, the header row is the collapse header.
fn project_truncation(
    span: &GroupSpan,
    participants: usize,
    hidden: usize,
    layout: &mut [EntryLayoutInfo],
    _max_visible: usize,
) {
    if span.expanded {
        // Expanded: entry 0 renders a collapse header above its own
        // content (slot height 2); the rest keep their heights.
        let cached = &mut layout[span.range.start];
        cached.group_collapse_header = true;
        cached.group_header_count = participants.min(u16::MAX as usize) as u16;
        cached.height = 2;
        for idx in span.range.clone().skip(1) {
            layout[idx].gap_after = 0;
        }
        return;
    }

    // Hide `hidden` participants; the first hidden one becomes the header.
    let first_hidden = participants - hidden;
    let header_idx = span.range.start + first_hidden;
    for idx in span.range.clone() {
        let cached = &mut layout[idx];
        if idx == header_idx {
            cached.group_header_count = hidden.min(u16::MAX as usize) as u16;
            cached.height = 1;
        } else if idx < header_idx {
            cached.height = 0;
        }
        // The visible tail (`first_hidden..participants` beyond the header)
        // keeps its heights — untouched here.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(id: u64, finished: bool, mode: DisplayMode) -> FoldEntry {
        FoldEntry {
            id,
            display_mode: mode,
            is_tool: true,
            tool_finished: finished,
            total_lines: 1,
        }
    }

    fn msg(id: u64, mode: DisplayMode, lines: usize) -> FoldEntry {
        FoldEntry {
            id,
            display_mode: mode,
            is_tool: false,
            tool_finished: true,
            total_lines: lines,
        }
    }

    /// Apply the fold projection the way the app does: heights are seeded
    /// from the entries' own expanded heights first, then the projection
    /// writes fold flags on top.
    fn apply(spans: &[GroupSpan], entries: &[FoldEntry], layout: &mut [EntryLayoutInfo], max_visible: usize) {
        for (i, e) in entries.iter().enumerate() {
            layout[i].height = e.total_lines.min(8) as u16 + 1;
        }
        project_to_layout(spans, layout, max_visible);
    }

    /// A run of ≥2 finished collapsed tools folds into one VerbRun span
    /// (grok `RunScan::folds` gate: runs of 1 stay unfolded).
    #[test]
    fn verb_run_folds_two_plus_finished_tools() {
        let entries = vec![
            tool(1, true, DisplayMode::Collapsed),
            tool(2, true, DisplayMode::Collapsed),
        ];
        let spans = scan(&entries, 1, &HashSet::new());
        assert_eq!(spans.len(), 1);
        assert_eq!(
            spans[0].kind,
            GroupKind::VerbRun { members: 2 },
            "two finished collapsed tools fold"
        );
        assert_eq!(spans[0].range, 0..2);
        assert!(!spans[0].expanded);

        let mut layout = vec![EntryLayoutInfo::default(); 2];
        apply(&spans, &entries, &mut layout, 1);
        assert_eq!(layout[0].height, 1, "header row");
        assert!(layout[0].verb_group_header);
        assert_eq!(layout[0].group_header_count, 2);
        assert_eq!(layout[1].height, 0, "claimed member hides");
    }

    /// A singleton tool run must not fold.
    #[test]
    fn single_tool_does_not_fold() {
        let entries = vec![tool(1, true, DisplayMode::Collapsed)];
        let spans = scan(&entries, 1, &HashSet::new());
        assert!(spans.is_empty());
    }

    /// Running / user-expanded tools never join a verb run.
    #[test]
    fn running_or_expanded_tools_do_not_fold() {
        let entries = vec![
            tool(1, false, DisplayMode::Collapsed), // still running
            tool(2, true, DisplayMode::Expanded),   // user-opened
            tool(3, true, DisplayMode::Collapsed),
        ];
        let spans = scan(&entries, 1, &HashSet::new());
        assert!(spans.is_empty(), "no two adjacent foldable tools");
    }

    /// An expanded group (keyed by first member id) unfolds every member:
    /// the header becomes the collapse header with height 2 and the first
    /// member keeps its own row below it.
    #[test]
    fn expanded_verb_group_reveals_members() {
        let entries = vec![
            tool(1, true, DisplayMode::Collapsed),
            tool(2, true, DisplayMode::Collapsed),
        ];
        let expanded: HashSet<u64> = [1].into_iter().collect();
        let spans = scan(&entries, 1, &expanded);
        assert_eq!(spans.len(), 1);
        assert!(spans[0].expanded);

        let mut layout = vec![EntryLayoutInfo::default(); 2];
        apply(&spans, &entries, &mut layout, 1);
        assert_eq!(layout[0].height, 2, "collapse header + first member row");
        assert!(layout[0].group_collapse_header);
        assert_eq!(layout[0].group_header_count, 2);
        assert_eq!(layout[1].height, 2, "member keeps its own rows");
    }

    /// Truncation: a run of collapsed entries longer than max_visible+1
    /// folds; the first hidden participant becomes the "N more" header.
    #[test]
    fn truncation_folds_long_collapsed_runs() {
        let entries = vec![
            msg(1, DisplayMode::Collapsed, 3),
            msg(2, DisplayMode::Collapsed, 3),
            msg(3, DisplayMode::Collapsed, 3),
        ];
        // max_visible=1 → fold only runs of 3+.
        let spans = scan(&entries, 1, &HashSet::new());
        assert_eq!(spans.len(), 1);
        assert_eq!(
            spans[0].kind,
            GroupKind::Truncation {
                participants: 3,
                hidden: 2
            }
        );

        let mut layout = vec![EntryLayoutInfo::default(); 3];
        apply(&spans, &entries, &mut layout, 1);
        // first_hidden = 3-2 = 1 → entry 1 becomes the header.
        assert_eq!(layout[0].height, 0, "older participant hides");
        assert_eq!(layout[1].height, 1);
        assert_eq!(layout[1].group_header_count, 2);
        assert_eq!(layout[2].height, 4, "visible tail keeps its own height");
    }

    /// Short collapsed runs stay unfolded.
    #[test]
    fn short_collapsed_runs_stay_unfolded() {
        let entries = vec![
            msg(1, DisplayMode::Collapsed, 3),
            msg(2, DisplayMode::Collapsed, 3),
        ];
        let spans = scan(&entries, 1, &HashSet::new());
        assert!(spans.is_empty(), "run of 2 with max_visible=1 must not fold");
    }

    /// Spans are disjoint and ordered; verb runs claim first so a verb run
    /// inside a collapsed run breaks the truncation scan.
    #[test]
    fn verb_run_claims_before_truncation() {
        let entries = vec![
            msg(1, DisplayMode::Collapsed, 3),
            msg(2, DisplayMode::Collapsed, 3),
            msg(3, DisplayMode::Collapsed, 3),
            tool(4, true, DisplayMode::Collapsed),
            tool(5, true, DisplayMode::Collapsed),
            msg(6, DisplayMode::Collapsed, 3),
        ];
        let spans = scan(&entries, 1, &HashSet::new());
        // Verb run (tools 4,5) claimed → the 3-message run before it folds
        // on its own and cannot span across the claimed entries.
        assert_eq!(spans.len(), 2, "one truncation run + one verb run");
        assert_eq!(spans[0].kind, GroupKind::Truncation { participants: 3, hidden: 2 });
        assert_eq!(spans[0].range, 0..3);
        assert_eq!(spans[1].kind, GroupKind::VerbRun { members: 2 });
        assert_eq!(spans[1].range, 3..5);
    }
}
