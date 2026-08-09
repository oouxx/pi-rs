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
    /// Resolved absolute path of the file.
    path: String,
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
}

impl EventTranslator {
    pub fn new(cwd: &str) -> Self {
        Self {
            cwd: cwd.to_string(),
            file_snapshots: HashMap::new(),
            bash_outputs: HashMap::new(),
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
                    let locations = self.tool_locations(&tool_name, &args, None);
                    let mut tc = acp::ToolCall::new(tool_call_id.clone(), tool_name.clone())
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
                    let mut fields = acp::ToolCallUpdateFields::new()
                        .status(acp::ToolCallStatus::Pending)
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
                    let mut fields = acp::ToolCallUpdateFields::new()
                        .status(acp::ToolCallStatus::Pending)
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
                let mut tc = acp::ToolCall::new(tool_call_id.clone(), tool_name.clone())
                    .kind(tool_kind(tool_name))
                    .status(acp::ToolCallStatus::InProgress)
                    .raw_input(args.clone());

                if is_bash_tool(tool_name) {
                    // Render bash as a display-only terminal (Zed executes
                    // `execute` tools with terminal content + terminal meta).
                    tc = tc.content(vec![acp::ToolCallContent::Terminal(acp::Terminal::new(
                        acp::TerminalId::new(tool_call_id.clone()),
                    ))]);
                    tc = tc.meta(bash_terminal_info_meta(tool_call_id, &self.cwd));
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
                                    path: resolved.to_string_lossy().to_string(),
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
                        tc = tc.locations(locs);
                    }
                }
                acp::SessionUpdate::ToolCall(tc)
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
                    let text = partial_text(partial_result);
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
                } else {
                    let text = partial_text(partial_result);
                    fields = fields.content(vec![acp::ToolCallContent::Content(acp::Content::new(
                        acp::ContentBlock::Text(acp::TextContent::new(text)),
                    ))]);
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
                let mut fields = acp::ToolCallUpdateFields::new()
                    .status(status)
                    .raw_output(result.clone());

                if is_bash_tool(tool_name) {
                    // Close the terminal with the exit code.
                    let exit_code = bash_exit_code(result, *is_error);
                    let update = acp::ToolCallUpdate::new(tool_call_id.clone(), fields)
                        .meta(bash_terminal_exit_meta(tool_call_id, exit_code));
                    acp::SessionUpdate::ToolCallUpdate(update)
                } else if tool_name == "edit" || tool_name == "write" {
                    // Emit a structured diff from the pre-mutation snapshot.
                    if let Some(snapshot) = self.file_snapshots.remove(tool_call_id) {
                        let new_text = std::fs::read_to_string(&snapshot.path).ok();
                        let diff = acp::Diff::new(
                            PathBuf::from(&snapshot.path),
                            new_text.unwrap_or_default(),
                        )
                        .old_text(snapshot.old_text);
                        fields = fields.content(vec![acp::ToolCallContent::Diff(diff)]);
                    }
                    acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                        tool_call_id.clone(),
                        fields,
                    ))
                } else {
                    let text = partial_text(result);
                    fields = fields.content(vec![acp::ToolCallContent::Content(acp::Content::new(
                        acp::ContentBlock::Text(acp::TextContent::new(text)),
                    ))]);
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
                    ..
                } = message
                else {
                    return None;
                };
                acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                    acp::ContentBlock::Text(acp::TextContent::new(format!("⚠️ {err}"))),
                ))
            }

            // Turn lifecycle / compaction / queue events have no ACP wire
            // equivalent — the prompt response itself signals turn completion.
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
fn is_bash_tool(tool_name: &str) -> bool {
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

/// Extract a plain-text representation from a tool result / partial result.
fn partial_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => {
            // Common shapes: { "text": "..." } or { "output": "..." }
            for key in ["text", "output", "content"] {
                if let Some(serde_json::Value::String(s)) = map.get(key) {
                    return s.clone();
                }
            }
            value.to_string()
        }
        _ => value.to_string(),
    }
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
fn bash_terminal_info_meta(tool_call_id: &str, cwd: &str) -> acp::Meta {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "terminal_info".to_string(),
        serde_json::json!({ "terminal_id": tool_call_id, "cwd": cwd }),
    );
    meta
}

/// ACP `_meta` for streamed bash output: `{ terminal_output: { terminal_id, data } }`.
fn bash_terminal_output_meta(tool_call_id: &str, data: &str) -> acp::Meta {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "terminal_output".to_string(),
        serde_json::json!({ "terminal_id": tool_call_id, "data": data }),
    );
    meta
}

/// ACP `_meta` for bash completion: `{ terminal_exit: { terminal_id, exit_code, signal } }`.
fn bash_terminal_exit_meta(tool_call_id: &str, exit_code: i64) -> acp::Meta {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "terminal_exit".to_string(),
        serde_json::json!({ "terminal_id": tool_call_id, "exit_code": exit_code, "signal": null }),
    );
    meta
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

        // Output deltas stream as terminal_output meta.
        let update = AgentSessionEvent::ToolExecutionUpdate {
            tool_call_id: "b1".into(),
            tool_name: "bash".into(),
            args: serde_json::json!({"command": "ls"}),
            partial_result: serde_json::json!({"text": "a.txt\nb.txt\n"}),
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
}
