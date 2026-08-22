//! Translate pi `AgentSessionEvent`s into ACP `SessionNotification`s.
//!
//! Mirrors the mapping done by the TS `pi-acp` adapter
//! (github.com/svkozak/pi-acp, `src/acp/session.ts` + `src/acp/translate/*`),
//! but natively in Rust: pi's streaming events become ACP `agent_message_chunk` /
//! `agent_thought_chunk` / `tool_call` / `tool_call_update` notifications.
//!
//! Beyond the plain event mapping, the translator adds the same client-facing
//! enrichments pi-acp does:
//! - **Tool call locations**: relative file paths in tool args are resolved
//!   against the session cwd and emitted as ACP `locations`, enabling
//!   follow-along features in clients like Zed. For `edit`, a 1-based line
//!   number is inferred from a unique `oldText` match in the pre-edit file.
//! - **Structured diffs**: for `edit`/`write`, the file is snapshotted before
//!   the tool runs and an ACP `Diff` (`oldText`/`newText`) is emitted on
//!   completion.
//! - **Bash terminals**: `bash` tool calls are rendered as ACP terminals
//!   (`terminal_info` / `terminal_output` / `terminal_exit` meta) so clients
//!   show a display-only terminal instead of a plain tool card.
//! - **Tool-call streaming**: tool calls are surfaced as soon as the model
//!   starts streaming their arguments (`toolcall_start`/`toolcall_delta`/
//!   `toolcall_end` inside `message_update`), not only when execution starts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use agent_client_protocol as acp;
use pi_agent_core::pi_ai_types::AssistantMessageEvent;

use crate::core::agent_session::AgentSessionEvent;

/// Snapshot of a file taken before a mutating tool (`edit`/`write`) ran.
struct FileSnapshot {
    /// Original path from the tool args (may be relative) — used in the ACP
    /// diff, matching pi-acp which emits the raw tool path.
    path: String,
    /// Resolved absolute path — used to read the file.
    resolved_path: PathBuf,
    /// Pre-mutation content (`None` when the file did not exist / unreadable).
    old_text: Option<String>,
}

/// Per-session state used to enrich tool-call notifications.
pub struct EventTranslator {
    /// Session working directory — relative tool paths are resolved against it.
    cwd: String,
    /// Pre-mutation file snapshots keyed by tool call id.
    file_snapshots: HashMap<String, FileSnapshot>,
    /// Accumulated bash output per tool call id (for delta computation).
    bash_outputs: HashMap<String, String>,
    /// Last-surfaced status per tool call id, so status transitions stay
    /// monotonic (pending → in_progress → completed/failed) and a tool that
    /// was already surfaced while the model streamed its args is not emitted
    /// as a second `tool_call` when execution starts (matching pi-acp's
    /// `currentToolCalls` map).
    tool_statuses: HashMap<String, acp::ToolCallStatus>,
    /// Epoch-ms start time per in-flight tool call id, recorded at
    /// `tool_execution_start` and used by the heartbeat to report
    /// `elapsed_ms` while a tool runs (clients have no other way to see
    /// progress for a silent, long-running command).
    tool_started_at: HashMap<String, u64>,
}

impl EventTranslator {
    pub fn new(cwd: &str) -> Self {
        Self {
            cwd: cwd.to_string(),
            file_snapshots: HashMap::new(),
            bash_outputs: HashMap::new(),
            tool_statuses: HashMap::new(),
            tool_started_at: HashMap::new(),
        }
    }

    /// Translate a pi session event into an ACP session notification, if any.
    ///
    /// Returns `None` for events that have no ACP wire equivalent (turn
    /// lifecycle, compaction, queue updates, etc.).
    pub fn translate(
        &mut self,
        session_id: &acp::SessionId,
        event: &AgentSessionEvent,
    ) -> Option<acp::SessionNotification> {
        let update = match event {
            // ── Assistant streaming ──────────────────────────────────────────
            AgentSessionEvent::MessageUpdate {
                assistant_message_event,
                ..
            } => match assistant_message_event {
                AssistantMessageEvent::TextDelta { delta, .. } => {
                    acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                        acp::ContentBlock::Text(acp::TextContent::new(delta.clone())),
                    ))
                }
                AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                    acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(
                        acp::ContentBlock::Text(acp::TextContent::new(delta.clone())),
                    ))
                }
                // Surface tool calls as soon as the model starts streaming
                // their arguments (matching pi-acp's `toolcall_start` /
                // `toolcall_delta` / `toolcall_end` handling).
                AssistantMessageEvent::ToolCallStart {
                    content_index,
                    partial,
                } => {
                    let (tool_call_id, tool_name, args) =
                        tool_call_at(partial, *content_index)?;
                    // Remember the tool was surfaced while streaming so
                    // `tool_execution_start` transitions it instead of
                    // emitting a duplicate `tool_call` (matching pi-acp).
                    self.tool_statuses
                        .insert(tool_call_id.clone(), acp::ToolCallStatus::Pending);
                    let locations = self.tool_locations(&tool_name, &args, None);
                    let mut tc = acp::ToolCall::new(
                        tool_call_id.clone(),
                        tool_title(&tool_name, &args),
                    )
                    .kind(tool_kind(&tool_name))
                    .status(acp::ToolCallStatus::Pending)
                    .raw_input(args.clone());
                    if let Some(locs) = locations {
                        tc = tc.locations(locs);
                    }
                    if is_bash_tool(&tool_name) {
                        tc = tc.content(vec![acp::ToolCallContent::Terminal(
                            acp::Terminal::new(acp::TerminalId::new(tool_call_id.clone())),
                        )]);
                        tc = tc.meta(bash_terminal_info_meta(&tool_call_id, &self.cwd));
                    }
                    acp::SessionUpdate::ToolCall(tc)
                }
                AssistantMessageEvent::ToolCallDelta {
                    content_index,
                    delta,
                    partial,
                } => {
                    let (tool_call_id, tool_name, args) =
                        tool_call_at(partial, *content_index)?;
                    let locations = self.tool_locations(&tool_name, &args, None);
                    let status = self
                        .tool_statuses
                        .get(&tool_call_id)
                        .cloned()
                        .unwrap_or(acp::ToolCallStatus::Pending);
                    // The title is set on the initial `tool_call` while args
                    // are still empty (`{}`), so refresh it here as the
                    // command streams in — otherwise Zed keeps showing the
                    // bare tool name "bash" instead of the full command.
                    let mut fields = acp::ToolCallUpdateFields::new()
                        .status(status)
                        .title(tool_title(&tool_name, &args))
                        .raw_input(args.clone());
                    if let Some(locs) = locations {
                        fields = fields.locations(locs);
                    }
                    let mut update = acp::ToolCallUpdate::new(tool_call_id, fields);
                    if is_bash_tool(&tool_name) {
                        let mut fields = update.fields.clone();
                        fields.content = Some(vec![acp::ToolCallContent::Terminal(acp::Terminal::new(
                            acp::TerminalId::new(update.tool_call_id.0.clone()),
                        ))]);
                        update = acp::ToolCallUpdate::new(update.tool_call_id.clone(), fields)
                            .meta(bash_terminal_info_meta(&update.tool_call_id.0, &self.cwd));
                    }
                    let _ = delta;
                    acp::SessionUpdate::ToolCallUpdate(update)
                }
                AssistantMessageEvent::ToolCallEnd {
                    content_index,
                    tool_call,
                    partial,
                } => {
                    let tool_call_id = tool_call.id.clone();
                    let tool_name = tool_call.name.clone();
                    let args = tool_call.arguments.clone();
                    let locations = self.tool_locations(&tool_name, &args, None);
                    let status = self
                        .tool_statuses
                        .get(&tool_call_id)
                        .cloned()
                        .unwrap_or(acp::ToolCallStatus::Pending);
                    // Final args are complete here; make sure the title shows
                    // the full command (the initial `tool_call` may have been
                    // emitted with empty args).
                    let mut fields = acp::ToolCallUpdateFields::new()
                        .status(status)
                        .title(tool_title(&tool_name, &args))
                        .raw_input(args.clone());
                    if let Some(locs) = locations {
                        fields = fields.locations(locs);
                    }
                    let mut update = acp::ToolCallUpdate::new(tool_call_id, fields);
                    if is_bash_tool(&tool_name) {
                        let mut fields = update.fields.clone();
                        fields.content = Some(vec![acp::ToolCallContent::Terminal(acp::Terminal::new(
                            acp::TerminalId::new(update.tool_call_id.0.clone()),
                        ))]);
                        update = acp::ToolCallUpdate::new(update.tool_call_id.clone(), fields)
                            .meta(bash_terminal_info_meta(&update.tool_call_id.0, &self.cwd));
                    }
                    let _ = (content_index, partial);
                    acp::SessionUpdate::ToolCallUpdate(update)
                }
                // Other delta/event types have no ACP wire equivalent.
                _ => return None,
            },

            // ── Tool execution ─────────────────────────────────────────────
            AgentSessionEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
                ..
            } => {
                // If the tool was already surfaced while the model streamed
                // its args, transition it to in_progress instead of emitting
                // a second `tool_call` (matching pi-acp's `currentToolCalls`
                // monotonic-status guard).
                let already_surfaced = self.tool_statuses.contains_key(tool_call_id);
                self.tool_statuses
                    .insert(tool_call_id.clone(), acp::ToolCallStatus::InProgress);
                // Record the execution start so the heartbeat can report
                // elapsed time while the tool runs.
                self.tool_started_at.insert(
                    tool_call_id.clone(),
                    now_epoch_ms(),
                );

                let title = tool_title(tool_name, args);
                let kind = tool_kind(tool_name);
                // Always carry the title here: args are complete at execution
                // start, so this is the authoritative title even if the
                // streaming path surfaced the tool with empty args (title
                // fell back to the bare tool name).
                let mut fields = acp::ToolCallUpdateFields::new()
                    .status(acp::ToolCallStatus::InProgress)
                    .title(title.clone())
                    .raw_input(args.clone());

                if is_bash_tool(tool_name) {
                    // Render bash as a display-only terminal (Zed executes
                    // `execute` tools with terminal content + terminal meta).
                    fields = fields.content(vec![acp::ToolCallContent::Terminal(
                        acp::Terminal::new(acp::TerminalId::new(tool_call_id.clone())),
                    )]);
                    let update = acp::ToolCallUpdate::new(tool_call_id.clone(), fields)
                        .meta(bash_terminal_info_meta(tool_call_id, &self.cwd));
                    if already_surfaced {
                        acp::SessionUpdate::ToolCallUpdate(update)
                    } else {
                        let tc = acp::ToolCall::new(tool_call_id.clone(), title)
                            .kind(kind)
                            .status(acp::ToolCallStatus::InProgress)
                            .raw_input(args.clone())
                            .content(update.fields.content.clone().unwrap_or_default())
                            .meta(update.meta.clone());
                        acp::SessionUpdate::ToolCall(tc)
                    }
                } else {
                    // Snapshot the file before a mutating tool runs so we can
                    // emit a structured diff on completion.
                    if tool_name == "edit" || tool_name == "write" {
                        if let Some(path) = get_tool_path(args) {
                            let resolved = resolve_path(&self.cwd, &path);
                            let old_text = std::fs::read_to_string(&resolved).ok();
                            self.file_snapshots.insert(
                                tool_call_id.clone(),
                                FileSnapshot {
                                    path: path.clone(),
                                    resolved_path: resolved,
                                    old_text,
                                },
                            );
                        }
                    }
                    // For `edit`, infer a 1-based line number from a unique
                    // `oldText` match in the pre-edit snapshot.
                    let line = if tool_name == "edit" {
                        self.file_snapshots
                            .get(tool_call_id)
                            .and_then(|s| s.old_text.as_deref())
                            .and_then(|text| {
                                get_edit_old_texts(args)
                                    .iter()
                                    .find_map(|needle| find_unique_line_number(text, needle))
                            })
                    } else {
                        None
                    };
                    if let Some(locs) = self.tool_locations(tool_name, args, line) {
                        fields = fields.locations(locs);
                    }
                    if already_surfaced {
                        acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                            tool_call_id.clone(),
                            fields,
                        ))
                    } else {
                        let mut tc = acp::ToolCall::new(tool_call_id.clone(), title)
                            .kind(kind)
                            .status(acp::ToolCallStatus::InProgress)
                            .raw_input(args.clone());
                        if let Some(locs) = fields.locations.clone() {
                            tc = tc.locations(locs);
                        }
                        acp::SessionUpdate::ToolCall(tc)
                    }
                }
            }
            AgentSessionEvent::ToolExecutionUpdate {
                tool_call_id,
                tool_name,
                partial_result,
                ..
            } => {
                let mut fields = acp::ToolCallUpdateFields::new()
                    .status(acp::ToolCallStatus::InProgress);
                if is_bash_tool(tool_name) {
                    // Stream bash output as terminal_output deltas.
                    let text = bash_result_text(partial_result);
                    let prev = self.bash_outputs.get(tool_call_id).cloned().unwrap_or_default();
                    let delta = if text.starts_with(&prev) {
                        text[prev.len()..].to_string()
                    } else {
                        text.clone()
                    };
                    self.bash_outputs.insert(tool_call_id.clone(), text);
                    let mut update = acp::ToolCallUpdate::new(tool_call_id.clone(), fields);
                    if !delta.is_empty() {
                        update = update.meta(bash_terminal_output_meta(tool_call_id, &delta));
                    }
                    acp::SessionUpdate::ToolCallUpdate(update)
                } else if tool_name == "edit" || tool_name == "write" {
                    // File-mutation tools don't stream intermediate content — a
                    // structured diff is emitted on completion (matching pi-acp).
                    acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                        tool_call_id.clone(),
                        fields,
                    ))
                } else {
                    let text = tool_result_to_text(partial_result);
                    fields = fields.content(vec![acp::ToolCallContent::Content(acp::Content::new(
                        acp::ContentBlock::Text(acp::TextContent::new(text)),
                    ))]);
                    fields = fields.raw_output(partial_result.clone());
                    acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                        tool_call_id.clone(),
                        fields,
                    ))
                }
            }
            AgentSessionEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
                ..
            } => {
                let status = if *is_error {
                    acp::ToolCallStatus::Failed
                } else {
                    acp::ToolCallStatus::Completed
                };
                // The tool call is done — drop its status so a future tool
                // with the same id starts fresh (matching pi-acp's
                // `cleanupToolCall`).
                self.tool_statuses.remove(tool_call_id);
                self.tool_started_at.remove(tool_call_id);
                let mut fields = acp::ToolCallUpdateFields::new()
                    .status(status)
                    .raw_output(result.clone());

                if is_bash_tool(tool_name) {
                    // Flush the remaining output into the terminal and close it
                    // with the exit code, matching pi-acp's `emitBashOutputUpdate`
                    // on `tool_execution_end`: a fast command may never have hit
                    // the 100ms update throttle, so the final result text (or its
                    // tail, e.g. the truncation notice / status line) must be
                    // delivered here — otherwise the terminal stays empty.
                    // Both keys go into ONE meta — `meta()` replaces rather than
                    // merges, so two chained calls would drop the output.
                    let text = bash_result_text(result);
                    let prev = self.bash_outputs.get(tool_call_id).cloned().unwrap_or_default();
                    let delta = if text.starts_with(&prev) {
                        text[prev.len()..].to_string()
                    } else {
                        text.clone()
                    };
                    // Tool is done — drop the accumulated output (matching
                    // pi-acp's `cleanupToolCall`).
                    self.bash_outputs.remove(tool_call_id);
                    let exit_code = bash_exit_code(result, *is_error);
                    let mut meta = serde_json::Map::new();
                    if !delta.is_empty() {
                        for (k, v) in bash_terminal_output_meta(tool_call_id, &delta) {
                            meta.insert(k, v);
                        }
                    }
                    for (k, v) in bash_terminal_exit_meta(tool_call_id, exit_code) {
                        meta.insert(k, v);
                    }
                    // Bash updates carry no rawOutput (matching pi-acp).
                    let update = acp::ToolCallUpdate::new(
                        tool_call_id.clone(),
                        acp::ToolCallUpdateFields::new().status(status),
                    )
                    .meta(meta);
                    acp::SessionUpdate::ToolCallUpdate(update)
                } else if tool_name == "edit" || tool_name == "write" {
                    // Emit a structured diff only when the tool succeeded and the
                    // file actually changed (matching pi-acp): a diff on an error
                    // or an unchanged file is noise, and a failed read must fall
                    // back to plain text instead of a bogus empty diff.
                    let mut has_diff = false;
                    if !*is_error {
                        if let Some(snapshot) = self.file_snapshots.remove(tool_call_id) {
                            if let Ok(new_text) = std::fs::read_to_string(&snapshot.resolved_path) {
                                if snapshot.old_text.is_none()
                                    || new_text != snapshot.old_text.as_deref().unwrap_or("")
                                {
                                    has_diff = true;
                                    let diff = acp::Diff::new(
                                        PathBuf::from(&snapshot.path),
                                        new_text,
                                    )
                                    .old_text(snapshot.old_text);
                                    fields = fields.content(vec![acp::ToolCallContent::Diff(diff)]);
                                }
                            }
                        }
                    }
                    if !has_diff {
                        // Fall back to plain text (and raw output) so the client
                        // still sees the tool result (matching pi-acp).
                        let text = tool_result_to_text(result);
                        if !text.is_empty() {
                            fields = fields.content(vec![acp::ToolCallContent::Content(
                                acp::Content::new(acp::ContentBlock::Text(acp::TextContent::new(
                                    text,
                                ))),
                            )]);
                        }
                        fields = fields.raw_output(result.clone());
                    }
                    acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                        tool_call_id.clone(),
                        fields,
                    ))
                } else {
                    let text = tool_result_to_text(result);
                    fields = fields.content(vec![acp::ToolCallContent::Content(acp::Content::new(
                        acp::ContentBlock::Text(acp::TextContent::new(text)),
                    ))]);
                    fields = fields.raw_output(result.clone());
                    acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                        tool_call_id.clone(),
                        fields,
                    ))
                }
            }

            // ── Session metadata ────────────────────────────────────────────
            AgentSessionEvent::SessionInfoChanged { name } => {
                let update = acp::SessionInfoUpdate::new().title(name.clone().unwrap_or_default());
                acp::SessionUpdate::SessionInfoUpdate(update)
            }

            // ── Assistant errors ───────────────────────────────────────────
            // ACP has no error stop reason, so surface the failure as a message
            // chunk — otherwise the client UI shows an empty turn with no
            // explanation (e.g. an LLM 402/insufficient-balance error).
            AgentSessionEvent::MessageEnd { message } => {
                let pi_agent_core::types::AgentMessage::Assistant {
                    error_message: Some(err),
                    stop_reason,
                    ..
                } = message
                else {
                    return None;
                };
                // A user-initiated abort is not an error — don't surface the
                // provider's "Request was aborted" message as a ⚠️ chunk.
                if stop_reason
                    == &Some(pi_agent_core::pi_ai_types::StopReason::Aborted)
                {
                    return None;
                }
                acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                    acp::ContentBlock::Text(acp::TextContent::new(format!("⚠️ {err}"))),
                ))
            }

            // ── Auto-retry / auto-compaction progress ───────────────────
            // pi-acp surfaces these as plain message chunks so the client UI
            // shows why the turn is taking longer (matching its
            // `auto_retry_start` / `auto_retry_end` / `auto_compaction_start` /
            // `auto_compaction_end` handling).
            AgentSessionEvent::AutoRetryStart {
                attempt,
                max_attempts,
                delay_ms,
                ..
            } => {
                let mut delay_seconds = (*delay_ms as f64 / 1000.0).round() as u64;
                if *delay_ms > 0 && delay_seconds == 0 {
                    delay_seconds = 1;
                }
                let text = format!(
                    "Retrying (attempt {attempt}/{max_attempts}, waiting {delay_seconds}s)..."
                );
                acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                    acp::ContentBlock::Text(acp::TextContent::new(text)),
                ))
            }
            AgentSessionEvent::AutoRetryEnd { .. } => {
                acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                    acp::ContentBlock::Text(acp::TextContent::new(
                        "Retry finished, resuming.".to_string(),
                    )),
                ))
            }
            AgentSessionEvent::CompactionStart { .. } => {
                acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                    acp::ContentBlock::Text(acp::TextContent::new(
                        "Context nearing limit, running automatic compaction...".to_string(),
                    )),
                ))
            }
            AgentSessionEvent::CompactionEnd { .. } => {
                acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                    acp::ContentBlock::Text(acp::TextContent::new(
                        "Automatic compaction finished; context was summarized to continue the session."
                            .to_string(),
                    )),
                ))
            }

            // Turn lifecycle / queue events have no ACP wire equivalent —
            // the prompt response itself signals turn completion.
            _ => return None,
        };

        Some(acp::SessionNotification::new(session_id.clone(), update))
    }

    /// Build ACP tool-call locations from tool args, resolving relative paths
    /// against the session cwd. Returns `None` when no path is present.
    fn tool_locations(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        line: Option<u32>,
    ) -> Option<Vec<acp::ToolCallLocation>> {
        let path = get_tool_path(args)?;
        let resolved = resolve_path(&self.cwd, &path);
        let mut loc = acp::ToolCallLocation::new(resolved);
        if let Some(l) = line {
            loc = loc.line(l);
        }
        let _ = tool_name;
        Some(vec![loc])
    }
}

/// Map a pi tool name to an ACP `ToolKind` so clients can pick icons/UI.
pub fn tool_kind(tool_name: &str) -> acp::ToolKind {
    match tool_name {
        "read" | "ls" | "find" | "grep" => acp::ToolKind::Read,
        "edit" | "write" => acp::ToolKind::Edit,
        "bash" => acp::ToolKind::Execute,
        "web_fetch" | "web_search" => acp::ToolKind::Fetch,
        _ => acp::ToolKind::Other,
    }
}

/// Whether the tool is the bash tool (case-insensitive, matching pi-acp).
pub(crate) fn is_bash_tool(tool_name: &str) -> bool {
    tool_name.eq_ignore_ascii_case("bash")
}

/// Extract the file path from tool args (`path` or `file_path`).
fn get_tool_path(args: &serde_json::Value) -> Option<String> {
    let obj = args.as_object()?;
    for key in ["path", "file_path"] {
        if let Some(serde_json::Value::String(s)) = obj.get(key) {
            if !s.is_empty() {
                return Some(s.clone());
            }
        }
    }
    None
}

/// Resolve a (possibly relative) tool path against the session cwd.
fn resolve_path(cwd: &str, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        Path::new(cwd).join(p)
    }
}

/// Collect the `oldText` needles from an `edit` tool's args, matching pi's
/// current edit schema `{ path, edits: [{ oldText, newText }] }` with legacy
/// top-level `oldText`/`newText` still accepted. Stringified `edits` are
/// parsed too.
fn get_edit_old_texts(args: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(obj) = args.as_object() else {
        return out;
    };

    if let Some(serde_json::Value::String(old)) = obj.get("oldText") {
        if !old.is_empty() {
            out.push(old.clone());
        }
    }

    let edits = match obj.get("edits") {
        Some(serde_json::Value::String(s)) => serde_json::from_str::<serde_json::Value>(s).ok(),
        Some(other) => Some(other.clone()),
        None => None,
    };
    if let Some(serde_json::Value::Array(items)) = edits {
        for item in items {
            if let Some(serde_json::Value::String(old)) =
                item.as_object().and_then(|o| o.get("oldText"))
            {
                if !old.is_empty() && !out.contains(old) {
                    out.push(old.clone());
                }
            }
        }
    }
    out
}

/// Find the 1-based line number of a *unique* occurrence of `needle` in
/// `text`. Returns `None` when the needle is absent or appears more than once
/// (matching pi-acp's `findUniqueLineNumber`).
fn find_unique_line_number(text: &str, needle: &str) -> Option<u32> {
    if needle.is_empty() {
        return None;
    }
    let first = text.find(needle)?;
    let after = &text[first + needle.len()..];
    if after.contains(needle) {
        return None;
    }
    let line = text[..first].bytes().filter(|&b| b == b'\n').count() as u32 + 1;
    Some(line)
}

/// Extract the tool call at `content_index` from a partial assistant message.
/// Returns `(id, name, arguments)`.
fn tool_call_at(
    partial: &pi_agent_core::pi_ai_types::AssistantMessage,
    content_index: usize,
) -> Option<(String, String, serde_json::Value)> {
    match partial.content.get(content_index) {
        Some(pi_agent_core::pi_ai_types::ContentBlock::ToolCall { id, name, arguments, .. }) => {
            Some((id.clone(), name.clone(), arguments.clone()))
        }
        _ => None,
    }
}

/// Extract a plain-text representation from a tool result / partial result,
/// matching pi-acp's `toolResultToText`: `details.diff` (edit's unified diff),
/// then `content` text blocks, then stdout/stderr/exit code, then JSON.
fn tool_result_to_text(value: &serde_json::Value) -> String {
    if value.is_null() {
        return String::new();
    }
    let obj = value.as_object();
    let details = obj.and_then(|o| o.get("details")).and_then(|d| d.as_object());

    // `details.diff` — pi's edit tool returns the unified diff there.
    if let Some(diff) = details.and_then(|d| d.get("diff")).and_then(|d| d.as_str()) {
        if !diff.trim().is_empty() {
            return diff.to_string();
        }
    }

    // `content: [{ type: "text", text: "..." }, ...]`
    if let Some(content) = obj.and_then(|o| o.get("content")).and_then(|c| c.as_array()) {
        let texts: Vec<&str> = content
            .iter()
            .filter_map(|c| {
                let c = c.as_object()?;
                if c.get("type").and_then(|t| t.as_str()) == Some("text") {
                    c.get("text").and_then(|t| t.as_str())
                } else {
                    None
                }
            })
            .collect();
        if !texts.is_empty() {
            return texts.join("");
        }
    }

    // stdout/stderr/exit code (bash often reports these in `details`).
    let get_str = |key: &str| -> Option<String> {
        details
            .and_then(|d| d.get(key))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                obj.and_then(|o| o.get(key))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
    };
    let stdout = get_str("stdout").or_else(|| get_str("output"));
    let stderr = get_str("stderr");
    let exit_code = obj
        .and_then(|o| o.get("exitCode"))
        .and_then(|v| v.as_i64())
        .or_else(|| obj.and_then(|o| o.get("code")).and_then(|v| v.as_i64()))
        .or_else(|| details.and_then(|d| d.get("exitCode")).and_then(|v| v.as_i64()))
        .or_else(|| details.and_then(|d| d.get("code")).and_then(|v| v.as_i64()));

    let has_stdout = stdout.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);
    let has_stderr = stderr.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);
    if has_stdout || has_stderr {
        let mut parts = Vec::new();
        if let Some(s) = stdout {
            if !s.trim().is_empty() {
                parts.push(s);
            }
        }
        if let Some(s) = stderr {
            if !s.trim().is_empty() {
                parts.push(format!("stderr:\n{s}"));
            }
        }
        if let Some(code) = exit_code {
            parts.push(format!("exit code: {code}"));
        }
        return parts.join("\n\n").trim_end().to_string();
    }

    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// Human-readable tool title, matching pi-acp: bash shows the command text
/// ("Run ls -la"), everything else shows the tool name.
fn tool_title(tool_name: &str, args: &serde_json::Value) -> String {
    if is_bash_tool(tool_name) {
        bash_command(args).unwrap_or_else(|| tool_name.to_string())
    } else {
        tool_name.to_string()
    }
}

/// Extract the shell command from bash tool args, matching pi-acp's
/// `bashCommand` (checks `command`/`cmd` at the top level and in nested
/// `args`/`input`/`rawInput`/`toolInput`/`details` objects).
pub(crate) fn bash_command(args: &serde_json::Value) -> Option<String> {
    fn find(v: &serde_json::Value) -> Option<String> {
        let obj = v.as_object()?;
        for key in ["command", "cmd"] {
            if let Some(s) = obj.get(key).and_then(|x| x.as_str()) {
                if !s.trim().is_empty() {
                    return Some(s.to_string());
                }
            }
        }
        for key in ["args", "input", "rawInput", "toolInput", "details"] {
            if let Some(nested) = obj.get(key) {
                if let Some(s) = find(nested) {
                    return Some(s);
                }
            }
        }
        None
    }
    find(args)
}

/// Extract plain text from a bash tool result, matching pi-acp's
/// `bashResultText` exactly: content text blocks first, then
/// stdout/`output`/stderr from `details` (or the top level), joined with
/// `\n` — **no** "stderr:" prefix or "exit code:" suffix (unlike
/// [`tool_result_to_text`], which is used for non-bash tools).
fn bash_result_text(value: &serde_json::Value) -> String {
    if value.is_null() {
        return String::new();
    }
    let obj = value.as_object();
    let details = obj.and_then(|o| o.get("details")).and_then(|d| d.as_object());

    // `content: [{ type: "text", text: "..." }, ...]`
    if let Some(content) = obj.and_then(|o| o.get("content")).and_then(|c| c.as_array()) {
        let texts: Vec<&str> = content
            .iter()
            .filter_map(|c| {
                let c = c.as_object()?;
                if c.get("type").and_then(|t| t.as_str()) == Some("text") {
                    c.get("text").and_then(|t| t.as_str())
                } else {
                    None
                }
            })
            .collect();
        if !texts.is_empty() {
            return texts.join("");
        }
    }

    let get_str = |key: &str| -> Option<String> {
        details
            .and_then(|d| d.get(key))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                obj.and_then(|o| o.get(key))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
    };
    let stdout = get_str("stdout").or_else(|| get_str("output"));
    let stderr = get_str("stderr");

    [stdout, stderr]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Best-effort exit code from a bash tool result (matching pi-acp's
/// `bashExitCode`): `details.exitCode` / `exitCode` / `details.code` / `code`,
/// falling back to 1 on error / 0 on success.
fn bash_exit_code(result: &serde_json::Value, is_error: bool) -> i64 {
    let obj = result.as_object();
    let details = obj.and_then(|o| o.get("details")).and_then(|d| d.as_object());
    for key in ["exitCode", "code"] {
        if let Some(serde_json::Value::Number(n)) = obj.and_then(|o| o.get(key)) {
            if let Some(i) = n.as_i64() {
                return i;
            }
        }
        if let Some(serde_json::Value::Number(n)) = details.and_then(|d| d.get(key)) {
            if let Some(i) = n.as_i64() {
                return i;
            }
        }
    }
    if is_error {
        1
    } else {
        0
    }
}

/// ACP `_meta` for a bash terminal: `{ terminal_info: { terminal_id, cwd } }`.
pub(crate) fn bash_terminal_info_meta(tool_call_id: &str, cwd: &str) -> acp::Meta {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "terminal_info".to_string(),
        serde_json::json!({ "terminal_id": tool_call_id, "cwd": cwd }),
    );
    meta
}

/// ACP `_meta` for streamed bash output: `{ terminal_output: { terminal_id, data } }`.
pub(crate) fn bash_terminal_output_meta(tool_call_id: &str, data: &str) -> acp::Meta {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "terminal_output".to_string(),
        serde_json::json!({ "terminal_id": tool_call_id, "data": data }),
    );
    meta
}

/// ACP `_meta` for bash completion: `{ terminal_exit: { terminal_id, exit_code, signal } }`.
pub(crate) fn bash_terminal_exit_meta(tool_call_id: &str, exit_code: i64) -> acp::Meta {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "terminal_exit".to_string(),
        serde_json::json!({ "terminal_id": tool_call_id, "exit_code": exit_code, "signal": null }),
    );
    meta
}

/// Current wall-clock time in epoch milliseconds.
fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl EventTranslator {
    /// Emit a heartbeat for every tool call currently `in_progress`: a
    /// `tool_call_update` carrying `_meta.elapsed_ms` since execution
    /// started. Called periodically (1s) while a turn runs so clients see
    /// progress even for a silent, long-running command (e.g. a stalled
    /// `git clone` that produces no output). Returns `None` when nothing is
    /// running.
    pub fn heartbeat(
        &mut self,
        session_id: &acp::SessionId,
    ) -> Option<acp::SessionNotification> {
        let now = now_epoch_ms();
        let mut running: Vec<(String, u64)> = Vec::new();
        for (id, status) in &self.tool_statuses {
            if *status == acp::ToolCallStatus::InProgress {
                if let Some(started) = self.tool_started_at.get(id) {
                    running.push((id.clone(), now.saturating_sub(*started)));
                }
            }
        }
        if running.is_empty() {
            return None;
        }
        // One notification per running tool (each carries its own id).
        let (id, elapsed_ms) = running.remove(0);
        let mut meta = serde_json::Map::new();
        meta.insert("elapsed_ms".to_string(), serde_json::json!(elapsed_ms));
        let update = acp::ToolCallUpdate::new(
            id,
            acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::InProgress),
        )
        .meta(meta);
        Some(acp::SessionNotification::new(
            session_id.clone(),
            acp::SessionUpdate::ToolCallUpdate(update),
        ))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use pi_agent_core::pi_ai_types::{AssistantMessage, StopReason, Usage};

    fn sample_assistant_message() -> AssistantMessage {
        AssistantMessage {
            content: vec![],
            api: "openai-completions".into(),
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        }
    }

    fn sid() -> acp::SessionId {
        acp::SessionId::new("s1")
    }

    #[test]
    fn text_delta_becomes_agent_message_chunk() {
        let event = AgentSessionEvent::MessageUpdate {
            message: serde_json::from_value(serde_json::json!({
                "role":"assistant","content":[],"api":"openai-completions","provider":"openai",
                "model":"gpt-5.5","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,
                "cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0}},
                "stopReason":"stop","timestamp":0
            })).unwrap(),
            assistant_message_event: AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "hello".into(),
                partial: sample_assistant_message(),
            },
        };
        let mut t = EventTranslator::new("/tmp");
        let notif = t.translate(&sid(), &event).expect("should translate");
        assert_eq!(notif.session_id, sid());
        match notif.update {
            acp::SessionUpdate::AgentMessageChunk(chunk) => {
                match chunk.content {
                    acp::ContentBlock::Text(t) => assert_eq!(t.text, "hello"),
                    _ => panic!("expected text content"),
                }
            }
            other => panic!("expected agent_message_chunk, got {other:?}"),
        }
    }

    #[test]
    fn thinking_delta_becomes_agent_thought_chunk() {
        let event = AgentSessionEvent::MessageUpdate {
            message: serde_json::from_value(serde_json::json!({
                "role":"assistant","content":[],"api":"openai-completions","provider":"openai",
                "model":"gpt-5.5","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,
                "cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0}},
                "stopReason":"stop","timestamp":0
            })).unwrap(),
            assistant_message_event: AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: "hmm".into(),
                partial: sample_assistant_message(),
            },
        };
        let mut t = EventTranslator::new("/tmp");
        let notif = t.translate(&sid(), &event).expect("should translate");
        assert!(matches!(notif.update, acp::SessionUpdate::AgentThoughtChunk(_)));
    }

    #[test]
    fn tool_execution_maps_to_tool_call_and_updates() {
        let start = AgentSessionEvent::ToolExecutionStart {
            tool_call_id: "t1".into(),
            tool_name: "read".into(),
            args: serde_json::json!({"path": "a.txt"}),
        };
        let mut t = EventTranslator::new("/tmp");
        let notif = t.translate(&sid(), &start).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCall(tc) => {
                assert_eq!(tc.tool_call_id.0.as_ref(), "t1");
                assert_eq!(tc.title, "read");
                assert_eq!(tc.kind, acp::ToolKind::Read);
                assert_eq!(tc.status, acp::ToolCallStatus::InProgress);
                assert_eq!(tc.raw_input, Some(serde_json::json!({"path": "a.txt"})));
            }
            other => panic!("expected tool_call, got {other:?}"),
        }

        let end = AgentSessionEvent::ToolExecutionEnd {
            tool_call_id: "t1".into(),
            tool_name: "read".into(),
            result: serde_json::json!({"text": "file contents"}),
            is_error: false,
        };
        let notif = t.translate(&sid(), &end).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCallUpdate(tcu) => {
                assert_eq!(tcu.tool_call_id.0.as_ref(), "t1");
                assert_eq!(tcu.fields.status, Some(acp::ToolCallStatus::Completed));
                assert_eq!(tcu.fields.raw_output, Some(serde_json::json!({"text": "file contents"})));
            }
            other => panic!("expected tool_call_update, got {other:?}"),
        }
    }

    /// Heartbeat: while a tool is in_progress, `heartbeat()` emits a
    /// `tool_call_update` carrying `_meta.elapsed_ms`; once the tool settles
    /// (or nothing is running) it returns `None`.
    #[test]
    fn heartbeat_reports_elapsed_for_in_progress_tools() {
        let start = AgentSessionEvent::ToolExecutionStart {
            tool_call_id: "t1".into(),
            tool_name: "bash".into(),
            args: serde_json::json!({"command": "git clone ..."}),
        };
        let mut t = EventTranslator::new("/tmp");

        // Nothing running yet → no heartbeat.
        assert!(t.heartbeat(&sid()).is_none());

        t.translate(&sid(), &start).expect("should translate");

        // In-progress tool → heartbeat with elapsed_ms meta.
        let notif = t.heartbeat(&sid()).expect("heartbeat while running");
        match notif.update {
            acp::SessionUpdate::ToolCallUpdate(tcu) => {
                assert_eq!(tcu.tool_call_id.0.as_ref(), "t1");
                assert_eq!(tcu.fields.status, Some(acp::ToolCallStatus::InProgress));
                let meta = tcu.meta.expect("heartbeat carries elapsed_ms meta");
                let elapsed = meta
                    .get("elapsed_ms")
                    .and_then(|v| v.as_u64())
                    .expect("elapsed_ms is a number");
                assert!(elapsed < 10_000, "elapsed should be small, got {elapsed}");
            }
            other => panic!("expected tool_call_update, got {other:?}"),
        }

        // After the tool settles, no more heartbeats.
        let end = AgentSessionEvent::ToolExecutionEnd {
            tool_call_id: "t1".into(),
            tool_name: "bash".into(),
            result: serde_json::json!({}),
            is_error: false,
        };
        t.translate(&sid(), &end).expect("should translate");
        assert!(t.heartbeat(&sid()).is_none());
    }

    /// Relative tool paths must be resolved against the session cwd and
    /// emitted as ACP locations (Zed follow-along).
    #[test]
    fn tool_call_emits_resolved_locations() {
        let start = AgentSessionEvent::ToolExecutionStart {
            tool_call_id: "t1".into(),
            tool_name: "read".into(),
            args: serde_json::json!({"path": "src/main.rs"}),
        };
        let mut t = EventTranslator::new("/proj");
        let notif = t.translate(&sid(), &start).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCall(tc) => {
                assert!(!tc.locations.is_empty(), "locations");
                let locs = &tc.locations;
                assert_eq!(locs[0].path, std::path::PathBuf::from("/proj/src/main.rs"));
                assert_eq!(locs[0].line, None);
            }
            other => panic!("expected tool_call, got {other:?}"),
        }
    }

    /// `edit` with a unique `oldText` must infer a 1-based line number from
    /// the pre-edit file snapshot and include it in the location.
    #[test]
    fn edit_infers_line_number_from_unique_oldtext() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "line1\nline2\nline3\n").expect("write");
        let path_str = file.to_string_lossy().to_string();

        let start = AgentSessionEvent::ToolExecutionStart {
            tool_call_id: "t1".into(),
            tool_name: "edit".into(),
            args: serde_json::json!({
                "path": path_str,
                "edits": [{ "oldText": "line2", "newText": "LINE2" }]
            }),
        };
        let mut t = EventTranslator::new("/proj");
        let notif = t.translate(&sid(), &start).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCall(tc) => {
                assert!(!tc.locations.is_empty(), "locations");
                let locs = &tc.locations;
                assert_eq!(locs.len(), 1);
                assert_eq!(locs[0].path, std::path::PathBuf::from(&path_str));
                assert_eq!(locs[0].line, Some(2), "line2 is on line 2");
            }
            other => panic!("expected tool_call, got {other:?}"),
        }
    }

    /// A non-unique `oldText` must not produce a line number (ambiguous).
    #[test]
    fn edit_ambiguous_oldtext_has_no_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "dup\ndup\n").expect("write");
        let path_str = file.to_string_lossy().to_string();

        let start = AgentSessionEvent::ToolExecutionStart {
            tool_call_id: "t1".into(),
            tool_name: "edit".into(),
            args: serde_json::json!({ "path": path_str, "oldText": "dup", "newText": "x" }),
        };
        let mut t = EventTranslator::new("/proj");
        let notif = t.translate(&sid(), &start).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCall(tc) => {
                assert!(!tc.locations.is_empty(), "locations");
                assert_eq!(tc.locations[0].line, None, "ambiguous oldText must not infer a line");
            }
            other => panic!("expected tool_call, got {other:?}"),
        }
    }

    /// `edit` completion must emit a structured ACP diff (oldText/newText).
    #[test]
    fn edit_completion_emits_structured_diff() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "before\n").expect("write");
        let path_str = file.to_string_lossy().to_string();

        let start = AgentSessionEvent::ToolExecutionStart {
            tool_call_id: "t1".into(),
            tool_name: "edit".into(),
            args: serde_json::json!({ "path": path_str, "oldText": "before", "newText": "after" }),
        };
        let mut t = EventTranslator::new("/proj");
        t.translate(&sid(), &start).expect("start");

        // Simulate the tool having applied the edit.
        std::fs::write(&file, "after\n").expect("write");

        let end = AgentSessionEvent::ToolExecutionEnd {
            tool_call_id: "t1".into(),
            tool_name: "edit".into(),
            result: serde_json::json!({"text": "ok"}),
            is_error: false,
        };
        let notif = t.translate(&sid(), &end).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCallUpdate(tcu) => {
                let content = tcu.fields.content.expect("content");
                assert_eq!(content.len(), 1);
                match &content[0] {
                    acp::ToolCallContent::Diff(diff) => {
                        assert_eq!(diff.path, std::path::PathBuf::from(&path_str));
                        assert_eq!(diff.old_text.as_deref(), Some("before\n"));
                        assert_eq!(diff.new_text, "after\n");
                    }
                    other => panic!("expected diff content, got {other:?}"),
                }
            }
            other => panic!("expected tool_call_update, got {other:?}"),
        }
    }

    /// Bash tool calls must be rendered as ACP terminals with terminal meta.
    #[test]
    fn bash_tool_renders_as_terminal() {
        let start = AgentSessionEvent::ToolExecutionStart {
            tool_call_id: "b1".into(),
            tool_name: "bash".into(),
            args: serde_json::json!({"command": "ls"}),
        };
        let mut t = EventTranslator::new("/proj");
        let notif = t.translate(&sid(), &start).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCall(tc) => {
                assert_eq!(tc.kind, acp::ToolKind::Execute);
                assert!(!tc.content.is_empty(), "content");
                assert!(matches!(&tc.content[0], acp::ToolCallContent::Terminal(_)));
                let meta = tc.meta.expect("meta");
                assert!(meta.contains_key("terminal_info"));
            }
            other => panic!("expected tool_call, got {other:?}"),
        }

        // Output deltas stream as terminal_output meta (bash reports output
        // as content text blocks, matching pi's bash tool).
        let update = AgentSessionEvent::ToolExecutionUpdate {
            tool_call_id: "b1".into(),
            tool_name: "bash".into(),
            args: serde_json::json!({"command": "ls"}),
            partial_result: serde_json::json!({
                "content": [{"type": "text", "text": "a.txt\nb.txt\n"}]
            }),
        };
        let notif = t.translate(&sid(), &update).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCallUpdate(tcu) => {
                let meta = tcu.meta.expect("meta");
                let out = meta.get("terminal_output").expect("terminal_output");
                assert_eq!(out["data"], "a.txt\nb.txt\n");
            }
            other => panic!("expected tool_call_update, got {other:?}"),
        }

        // Completion closes the terminal with the exit code.
        let end = AgentSessionEvent::ToolExecutionEnd {
            tool_call_id: "b1".into(),
            tool_name: "bash".into(),
            result: serde_json::json!({"details": {"exitCode": 0}}),
            is_error: false,
        };
        let notif = t.translate(&sid(), &end).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCallUpdate(tcu) => {
                let meta = tcu.meta.expect("meta");
                let exit = meta.get("terminal_exit").expect("terminal_exit");
                assert_eq!(exit["exit_code"], 0);
            }
            other => panic!("expected tool_call_update, got {other:?}"),
        }
    }

    /// A fast bash command that never hit the 100ms update throttle still
    /// delivers its full output: `tool_execution_end` must flush the remaining
    /// text as a `terminal_output` delta alongside `terminal_exit` (matching
    /// pi-acp's `emitBashOutputUpdate` on `tool_execution_end`). Otherwise the
    /// terminal renders empty for any command that finishes in <100ms.
    #[test]
    fn bash_completion_flushes_unstreamed_output() {
        let mut t = EventTranslator::new("/proj");

        // No ToolExecutionUpdate ever fired (fast command).
        let end = AgentSessionEvent::ToolExecutionEnd {
            tool_call_id: "b1".into(),
            tool_name: "bash".into(),
            result: serde_json::json!({
                "content": [{"type": "text", "text": "total 0\n"}],
                "details": {}
            }),
            is_error: false,
        };
        let notif = t.translate(&sid(), &end).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCallUpdate(tcu) => {
                let meta = tcu.meta.expect("meta");
                let out = meta.get("terminal_output").expect("terminal_output");
                assert_eq!(
                    out["data"], "total 0\n",
                    "unstreamed output must be flushed at completion"
                );
                let exit = meta.get("terminal_exit").expect("terminal_exit");
                assert_eq!(exit["exit_code"], 0);
                assert!(
                    tcu.fields.raw_output.is_none(),
                    "bash updates must not carry rawOutput (matching pi-acp)"
                );
            }
            other => panic!("expected tool_call_update, got {other:?}"),
        }
        assert!(
            t.bash_outputs.is_empty(),
            "bash output state must be cleaned up at completion"
        );
    }

    /// When output was already streamed, `tool_execution_end` only appends the
    /// tail (e.g. the "Command exited with code N" status line the bash tool
    /// appends to the error text), matching pi-acp's delta logic — no duplicate
    /// full re-send.
    #[test]
    fn bash_completion_appends_only_status_tail() {
        let mut t = EventTranslator::new("/proj");

        // Output streamed while running.
        let update = AgentSessionEvent::ToolExecutionUpdate {
            tool_call_id: "b1".into(),
            tool_name: "bash".into(),
            args: serde_json::json!({"command": "ls"}),
            partial_result: serde_json::json!({
                "content": [{"type": "text", "text": "a.txt\n"}]
            }),
        };
        t.translate(&sid(), &update).expect("should translate");

        // Final result: streamed text + status suffix.
        let end = AgentSessionEvent::ToolExecutionEnd {
            tool_call_id: "b1".into(),
            tool_name: "bash".into(),
            result: serde_json::json!({
                "content": [{"type": "text", "text": "a.txt\n\nCommand exited with code 1"}],
                "details": {}
            }),
            is_error: true,
        };
        let notif = t.translate(&sid(), &end).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCallUpdate(tcu) => {
                let meta = tcu.meta.expect("meta");
                let out = meta.get("terminal_output").expect("terminal_output");
                assert_eq!(
                    out["data"], "\nCommand exited with code 1",
                    "only the unstreamed tail must be sent"
                );
                let exit = meta.get("terminal_exit").expect("terminal_exit");
                assert_eq!(exit["exit_code"], 1);
            }
            other => panic!("expected tool_call_update, got {other:?}"),
        }
    }

    /// The bash tool emits an initial EMPTY update (`{"content": [],
    /// "details": null}`) before any output arrives. That must never leak
    /// into the terminal as raw JSON: pi-acp's `bashResultText` returns ""
    /// for an empty content array (no pretty-printed JSON fallback), so the
    /// delta is empty and no `terminal_output` is emitted.
    #[test]
    fn bash_empty_initial_update_does_not_leak_json() {
        let mut t = EventTranslator::new("/proj");

        let update = AgentSessionEvent::ToolExecutionUpdate {
            tool_call_id: "b1".into(),
            tool_name: "bash".into(),
            args: serde_json::json!({"command": "git diff"}),
            partial_result: serde_json::json!({
                "content": [],
                "details": null
            }),
        };
        let notif = t.translate(&sid(), &update).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCallUpdate(tcu) => {
                assert!(
                    tcu.meta.is_none(),
                    "empty update must not emit any terminal_output meta, got {:?}",
                    tcu.meta
                );
            }
            other => panic!("expected tool_call_update, got {other:?}"),
        }

        // The follow-up update with real output must carry ONLY the output
        // text — no JSON prefix.
        let update = AgentSessionEvent::ToolExecutionUpdate {
            tool_call_id: "b1".into(),
            tool_name: "bash".into(),
            args: serde_json::json!({"command": "git diff"}),
            partial_result: serde_json::json!({
                "content": [{"type": "text", "text": "README.md | 10 +++\n"}],
                "details": null
            }),
        };
        let notif = t.translate(&sid(), &update).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCallUpdate(tcu) => {
                let meta = tcu.meta.expect("meta");
                let out = meta.get("terminal_output").expect("terminal_output");
                assert_eq!(out["data"], "README.md | 10 +++\n");
            }
            other => panic!("expected tool_call_update, got {other:?}"),
        }
    }

    /// Tool calls must be surfaced while the model is still streaming their
    /// arguments (toolcall_start inside message_update).
    #[test]
    fn toolcall_start_surfaces_tool_call_while_streaming() {
        let partial = AssistantMessage {
            content: vec![pi_agent_core::pi_ai_types::ContentBlock::ToolCall {
                id: "tc1".into(),
                name: "read".into(),
                arguments: serde_json::json!({"path": "a.txt"}),
                thought_signature: None,
            }],
            api: "openai-completions".into(),
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        };
        let event = AgentSessionEvent::MessageUpdate {
            message: serde_json::from_value(serde_json::json!({
                "role":"assistant","content":[],"api":"openai-completions","provider":"openai",
                "model":"gpt-5.5","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,
                "cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0}},
                "stopReason":"stop","timestamp":0
            })).unwrap(),
            assistant_message_event: AssistantMessageEvent::ToolCallStart {
                content_index: 0,
                partial,
            },
        };
        let mut t = EventTranslator::new("/proj");
        let notif = t.translate(&sid(), &event).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCall(tc) => {
                assert_eq!(tc.tool_call_id.0.as_ref(), "tc1");
                assert_eq!(tc.status, acp::ToolCallStatus::Pending);
                assert!(!tc.locations.is_empty(), "locations");
                assert_eq!(tc.locations[0].path, std::path::PathBuf::from("/proj/a.txt"));
            }
            other => panic!("expected tool_call, got {other:?}"),
        }
    }

    #[test]
    fn lifecycle_events_are_not_translated() {
        let mut t = EventTranslator::new("/tmp");
        for event in [
            AgentSessionEvent::AgentStart,
            AgentSessionEvent::TurnStart,
            AgentSessionEvent::AgentSettled,
        ] {
            assert!(t.translate(&sid(), &event).is_none(), "unexpected translation");
        }
    }

    /// An assistant message that ended with an error must be surfaced to the
    /// client as a message chunk — otherwise the UI shows an empty turn with
    /// no explanation (e.g. LLM 402 insufficient-balance).
    #[test]
    fn assistant_error_message_is_translated() {
        let event = AgentSessionEvent::MessageEnd {
            message: pi_agent_core::types::AgentMessage::Assistant {
                content: vec![],
                api: "openai-completions".into(),
                provider: "deepseek".into(),
                model: "deepseek-v4-pro".into(),
                usage: pi_agent_core::pi_ai_types::Usage::default(),
                stop_reason: Some(pi_agent_core::pi_ai_types::StopReason::Error),
                error_message: Some("OpenAI API error 402 Payment Required".into()),
                timestamp: 0,
            },
        };
        let mut t = EventTranslator::new("/tmp");
        let notif = t.translate(&sid(), &event).expect("error must be translated");
        match notif.update {
            acp::SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
                acp::ContentBlock::Text(t) => {
                    assert!(t.text.contains("402"), "error text must be surfaced");
                }
                _ => panic!("expected text content"),
            },
            other => panic!("expected agent_message_chunk, got {other:?}"),
        }
    }

    /// A normal (non-error) assistant message end has no ACP wire equivalent.
    #[test]
    fn normal_message_end_is_not_translated() {
        let event = AgentSessionEvent::MessageEnd {
            message: pi_agent_core::types::AgentMessage::Assistant {
                content: vec![],
                api: "openai-completions".into(),
                provider: "deepseek".into(),
                model: "deepseek-v4-pro".into(),
                usage: pi_agent_core::pi_ai_types::Usage::default(),
                stop_reason: Some(pi_agent_core::pi_ai_types::StopReason::Stop),
                error_message: None,
                timestamp: 0,
            },
        };
        let mut t = EventTranslator::new("/tmp");
        assert!(t.translate(&sid(), &event).is_none());
    }

    #[test]
    fn find_unique_line_number_helpers() {
        assert_eq!(find_unique_line_number("a\nb\nc\n", "b"), Some(2));
        assert_eq!(find_unique_line_number("a\nb\nb\n", "b"), None);
        assert_eq!(find_unique_line_number("abc", "x"), None);
        assert_eq!(find_unique_line_number("abc", ""), None);
        assert_eq!(find_unique_line_number("abc", "abc"), Some(1));
    }

    #[test]
    fn get_edit_old_texts_handles_edits_array_and_stringified() {
        let args = serde_json::json!({
            "path": "a.txt",
            "edits": "[{\"oldText\":\"x\",\"newText\":\"y\"},{\"oldText\":\"z\",\"newText\":\"w\"}]"
        });
        let old_texts = get_edit_old_texts(&args);
        assert_eq!(old_texts, vec!["x", "z"]);
    }

    /// A failed edit must not emit a diff (the file may be unchanged); it
    /// falls back to plain text + raw output (matching pi-acp).
    #[test]
    fn edit_failure_falls_back_to_text_not_diff() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "before\n").expect("write");
        let path_str = file.to_string_lossy().to_string();

        let start = AgentSessionEvent::ToolExecutionStart {
            tool_call_id: "t1".into(),
            tool_name: "edit".into(),
            args: serde_json::json!({ "path": path_str, "oldText": "before", "newText": "after" }),
        };
        let mut t = EventTranslator::new("/proj");
        t.translate(&sid(), &start).expect("start");

        // Tool failed; file unchanged.
        let end = AgentSessionEvent::ToolExecutionEnd {
            tool_call_id: "t1".into(),
            tool_name: "edit".into(),
            result: serde_json::json!({"content": [{"type": "text", "text": "edit failed"}]}),
            is_error: true,
        };
        let notif = t.translate(&sid(), &end).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCallUpdate(tcu) => {
                assert_eq!(tcu.fields.status, Some(acp::ToolCallStatus::Failed));
                let content = tcu.fields.content.expect("content");
                assert!(
                    matches!(&content[0], acp::ToolCallContent::Content(_)),
                    "failed edit must fall back to text, got {content:?}"
                );
                assert!(tcu.fields.raw_output.is_some(), "raw output expected");
            }
            other => panic!("expected tool_call_update, got {other:?}"),
        }
    }

    /// An edit that did not change the file must not emit a diff (matching
    /// pi-acp's `newText !== oldText` check).
    #[test]
    fn edit_unchanged_file_has_no_diff() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "same\n").expect("write");
        let path_str = file.to_string_lossy().to_string();

        let start = AgentSessionEvent::ToolExecutionStart {
            tool_call_id: "t1".into(),
            tool_name: "edit".into(),
            args: serde_json::json!({ "path": path_str, "oldText": "same", "newText": "same" }),
        };
        let mut t = EventTranslator::new("/proj");
        t.translate(&sid(), &start).expect("start");

        // Tool "succeeded" but wrote identical content.
        let end = AgentSessionEvent::ToolExecutionEnd {
            tool_call_id: "t1".into(),
            tool_name: "edit".into(),
            result: serde_json::json!({"content": [{"type": "text", "text": "ok"}]}),
            is_error: false,
        };
        let notif = t.translate(&sid(), &end).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCallUpdate(tcu) => {
                let content = tcu.fields.content.expect("content");
                assert!(
                    matches!(&content[0], acp::ToolCallContent::Content(_)),
                    "unchanged file must not emit a diff, got {content:?}"
                );
            }
            other => panic!("expected tool_call_update, got {other:?}"),
        }
    }

    /// A user-initiated abort must not surface the provider's
    /// "Request was aborted" message as a ⚠️ chunk.
    #[test]
    fn aborted_message_end_is_not_translated() {
        let event = AgentSessionEvent::MessageEnd {
            message: pi_agent_core::types::AgentMessage::Assistant {
                content: vec![],
                api: "openai-completions".into(),
                provider: "openai".into(),
                model: "gpt-5.5".into(),
                usage: pi_agent_core::pi_ai_types::Usage::default(),
                stop_reason: Some(pi_agent_core::pi_ai_types::StopReason::Aborted),
                error_message: Some("Request was aborted".to_string()),
                timestamp: 0,
            },
        };
        let mut t = EventTranslator::new("/proj");
        assert!(
            t.translate(&sid(), &event).is_none(),
            "aborted message must not be translated"
        );
    }

    /// Bash tool calls get a descriptive title with the command text
    /// (matching pi-acp's `bashCommand`), other tools keep the tool name.
    #[test]
    fn bash_tool_call_has_command_title() {
        let start = AgentSessionEvent::ToolExecutionStart {
            tool_call_id: "b1".into(),
            tool_name: "bash".into(),
            args: serde_json::json!({"command": "ls -la"}),
        };
        let mut t = EventTranslator::new("/proj");
        let notif = t.translate(&sid(), &start).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCall(tc) => {
                assert_eq!(tc.title, "ls -la");
            }
            other => panic!("expected tool_call, got {other:?}"),
        }

        // Non-bash tools keep the tool name as title.
        let read = AgentSessionEvent::ToolExecutionStart {
            tool_call_id: "r1".into(),
            tool_name: "read".into(),
            args: serde_json::json!({"path": "a.txt"}),
        };
        let notif = t.translate(&sid(), &read).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCall(tc) => {
                assert_eq!(tc.title, "read");
            }
            other => panic!("expected tool_call, got {other:?}"),
        }
    }

    /// When the model streams a bash tool call, the initial `tool_call` is
    /// emitted with empty args (title falls back to "bash"); the title must
    /// be refreshed on `tool_call_delta` / `tool_call_end` once the command
    /// has streamed in — otherwise Zed keeps showing the bare tool name.
    #[test]
    fn bash_title_updates_after_args_stream_in() {
        let mut t = EventTranslator::new("/proj");

        // 1. ToolCallStart: args are still `{}` → title falls back to "bash".
        let partial = AssistantMessage {
            content: vec![pi_agent_core::pi_ai_types::ContentBlock::ToolCall {
                id: "b1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({}),
                thought_signature: None,
            }],
            api: "openai-completions".into(),
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        };
        let start = AgentSessionEvent::MessageUpdate {
            message: serde_json::from_value(serde_json::json!({
                "role":"assistant","content":[],"api":"openai-completions","provider":"openai",
                "model":"gpt-5.5","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,
                "cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0}},
                "stopReason":"stop","timestamp":0
            }))
            .unwrap(),
            assistant_message_event: AssistantMessageEvent::ToolCallStart {
                content_index: 0,
                partial,
            },
        };
        let notif = t.translate(&sid(), &start).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCall(tc) => {
                assert_eq!(tc.title, "bash", "empty args must fall back to tool name");
            }
            other => panic!("expected tool_call, got {other:?}"),
        }

        // 2. ToolCallDelta: the command has streamed in → title must update.
        let partial = AssistantMessage {
            content: vec![pi_agent_core::pi_ai_types::ContentBlock::ToolCall {
                id: "b1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "ls -la"}),
                thought_signature: None,
            }],
            api: "openai-completions".into(),
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        };
        let delta = AgentSessionEvent::MessageUpdate {
            message: serde_json::from_value(serde_json::json!({
                "role":"assistant","content":[],"api":"openai-completions","provider":"openai",
                "model":"gpt-5.5","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,
                "cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0}},
                "stopReason":"stop","timestamp":0
            }))
            .unwrap(),
            assistant_message_event: AssistantMessageEvent::ToolCallDelta {
                content_index: 0,
                delta: "{\"command\":\"ls -la\"}".into(),
                partial,
            },
        };
        let notif = t.translate(&sid(), &delta).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCallUpdate(tcu) => {
                assert_eq!(
                    tcu.fields.title.as_deref(),
                    Some("ls -la"),
                    "title must be refreshed once the command streams in"
                );
            }
            other => panic!("expected tool_call_update, got {other:?}"),
        }
    }

    /// File-mutation tools (edit/write) don't stream intermediate content
    /// during execution — only the final diff (matching pi-acp).
    #[test]
    fn file_mutation_update_has_no_content() {
        let update = AgentSessionEvent::ToolExecutionUpdate {
            tool_call_id: "t1".into(),
            tool_name: "edit".into(),
            args: serde_json::json!({"path": "a.txt"}),
            partial_result: serde_json::json!({"content": [{"type": "text", "text": "partial"}]}),
        };
        let mut t = EventTranslator::new("/proj");
        let notif = t.translate(&sid(), &update).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCallUpdate(tcu) => {
                assert!(
                    tcu.fields.content.is_none(),
                    "edit update must not stream content, got {:?}",
                    tcu.fields.content
                );
                assert!(
                    tcu.fields.raw_output.is_none(),
                    "edit update must not stream raw output"
                );
            }
            other => panic!("expected tool_call_update, got {other:?}"),
        }
    }

    /// A tool surfaced while the model streamed its args must be *transitioned*
    /// (tool_call_update) when execution starts, not re-emitted as a second
    /// tool_call (matching pi-acp's monotonic-status guard).
    #[test]
    fn tool_execution_start_transitions_already_surfaced_tool() {
        let mut t = EventTranslator::new("/proj");

        // 1) Model streams the tool call args → tool_call (pending).
        let partial = AssistantMessage {
            content: vec![pi_agent_core::pi_ai_types::ContentBlock::ToolCall {
                id: "tc1".into(),
                name: "read".into(),
                arguments: serde_json::json!({"path": "a.txt"}),
                thought_signature: None,
            }],
            api: "openai-completions".into(),
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        };
        let start = AgentSessionEvent::MessageUpdate {
            message: serde_json::from_value(serde_json::json!({
                "role":"assistant","content":[],"api":"openai-completions","provider":"openai",
                "model":"gpt-5.5","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,
                "cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0}},
                "stopReason":"stop","timestamp":0
            })).unwrap(),
            assistant_message_event: AssistantMessageEvent::ToolCallStart {
                content_index: 0,
                partial,
            },
        };
        assert!(matches!(
            t.translate(&sid(), &start).expect("start").update,
            acp::SessionUpdate::ToolCall(_)
        ));

        // 2) Execution starts → must be a tool_call_update, not a new tool_call.
        let exec = AgentSessionEvent::ToolExecutionStart {
            tool_call_id: "tc1".into(),
            tool_name: "read".into(),
            args: serde_json::json!({"path": "a.txt"}),
        };
        match t.translate(&sid(), &exec).expect("exec").update {
            acp::SessionUpdate::ToolCallUpdate(tcu) => {
                assert_eq!(tcu.tool_call_id.0.as_ref(), "tc1");
                assert_eq!(tcu.fields.status, Some(acp::ToolCallStatus::InProgress));
            }
            other => panic!("expected tool_call_update, got {other:?}"),
        }

        // 3) Completion → completed, and the status map is cleaned up so a
        //    future tool with the same id starts fresh.
        let end = AgentSessionEvent::ToolExecutionEnd {
            tool_call_id: "tc1".into(),
            tool_name: "read".into(),
            result: serde_json::json!({"text": "ok"}),
            is_error: false,
        };
        match t.translate(&sid(), &end).expect("end").update {
            acp::SessionUpdate::ToolCallUpdate(tcu) => {
                assert_eq!(tcu.fields.status, Some(acp::ToolCallStatus::Completed));
            }
            other => panic!("expected tool_call_update, got {other:?}"),
        }
        assert!(
            t.tool_statuses.is_empty(),
            "tool status must be cleaned up after completion"
        );
    }

    /// Auto-retry and auto-compaction progress must be surfaced as message
    /// chunks (matching pi-acp's auto_retry_* / auto_compaction_* handling).
    #[test]
    fn auto_retry_and_compaction_are_surfaced() {
        let mut t = EventTranslator::new("/proj");

        let retry = AgentSessionEvent::AutoRetryStart {
            attempt: 2,
            max_attempts: 3,
            delay_ms: 1500,
            error_message: "rate limited".into(),
        };
        match t.translate(&sid(), &retry).expect("retry").update {
            acp::SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
                acp::ContentBlock::Text(t) => {
                    assert_eq!(t.text, "Retrying (attempt 2/3, waiting 2s)...");
                }
                _ => panic!("expected text content"),
            },
            other => panic!("expected agent_message_chunk, got {other:?}"),
        }

        let retry_end = AgentSessionEvent::AutoRetryEnd {
            success: true,
            attempt: 2,
            final_error: None,
        };
        match t.translate(&sid(), &retry_end).expect("retry end").update {
            acp::SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
                acp::ContentBlock::Text(t) => assert_eq!(t.text, "Retry finished, resuming."),
                _ => panic!("expected text content"),
            },
            other => panic!("expected agent_message_chunk, got {other:?}"),
        }

        let comp = AgentSessionEvent::CompactionStart {
            reason: crate::core::agent_session::CompactionReason::Threshold,
        };
        match t.translate(&sid(), &comp).expect("compaction").update {
            acp::SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
                acp::ContentBlock::Text(t) => {
                    assert!(t.text.contains("running automatic compaction"));
                }
                _ => panic!("expected text content"),
            },
            other => panic!("expected agent_message_chunk, got {other:?}"),
        }
    }
}
