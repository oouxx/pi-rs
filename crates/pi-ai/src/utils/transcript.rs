//! Mid-conversation system/tool transcript helpers (port of
//! `packages/ai/src/utils/transcript.ts`). After P1-7 the prompt and tool
//! declarations live in the transcript's system messages; providers replay them
//! with these helpers instead of reading `Context.systemPrompt` / `Context.tools`.
//!
//! Messages are `serde_json::Value` because TS's helpers are structurally typed.
//! Note: `serde_json` iterates keys sorted (TS uses insertion order), so section
//! ordering follows sorted keys — not observable today (no prompt sections yet).
use std::collections::HashMap;

use serde_json::{json, Value};

use crate::types::{Tool, ToolReference};

fn is_system(message: &Value) -> bool {
    message.get("role").and_then(|r| r.as_str()) == Some("system")
}

fn system_content_text(content: &crate::types::SystemContent, separator: &str) -> String {
    match content {
        crate::types::SystemContent::Text(text) => text.clone(),
        crate::types::SystemContent::Blocks(blocks) => blocks
            .iter()
            .filter(|b| matches!(b, crate::types::ContentBlock::Text { .. }))
            .filter_map(|b| match b {
                crate::types::ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(separator),
    }
}

/// TS `getSystemMessageText` for a typed system message (content, then the
/// non-null sections, blank-line joined).
#[must_use]
pub fn typed_system_message_text(message: &crate::types::Message) -> String {
    let crate::types::Message::System {
        content, sections, ..
    } = message
    else {
        return String::new();
    };
    let mut parts: Vec<String> = vec![system_content_text(content, "\n")];
    if let Some(sections) = sections {
        for value in sections.values().flatten() {
            parts.push(value.clone());
        }
    }
    parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// TS `renderSystemMessageUpdate` for a typed system message.
#[must_use]
pub fn typed_render_system_message_update(message: &crate::types::Message) -> String {
    let crate::types::Message::System {
        content, sections, ..
    } = message
    else {
        return String::new();
    };
    let mut parts: Vec<String> = Vec::new();
    let text = system_content_text(content, "\n");
    if !text.is_empty() {
        parts.push(text);
    }
    if let Some(sections) = sections {
        for (name, value) in sections {
            match value {
                Some(text) => parts.push(format!(
                    "Updated system prompt section \"{name}\":\n\n{text}"
                )),
                None => parts.push(format!("Removed system prompt section \"{name}\".")),
            }
        }
    }
    parts.join("\n\n")
}

/// TS `contentText(content, separator)`: string content verbatim, otherwise the
/// joined text blocks.
pub fn content_text_value(content: &Value, separator: &str) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    content
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join(separator)
        })
        .unwrap_or_default()
}

/// TS `getSystemMessageText`: content followed by the non-null sections.
pub fn get_system_message_text(message: &Value) -> String {
    let mut parts: Vec<String> = vec![content_text_value(
        message.get("content").unwrap_or(&Value::Null),
        "\n",
    )];
    if let Some(sections) = message.get("sections").and_then(|s| s.as_object()) {
        for value in sections.values() {
            if let Some(text) = value.as_str() {
                parts.push(text.to_string());
            }
        }
    }
    parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// TS `renderSystemMessageUpdate`: render a later system message for providers
/// that accept system messages mid-conversation.
pub fn render_system_message_update(message: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    let text = content_text_value(message.get("content").unwrap_or(&Value::Null), "\n");
    if !text.is_empty() {
        parts.push(text);
    }
    if let Some(sections) = message.get("sections").and_then(|s| s.as_object()) {
        for (name, value) in sections {
            match value.as_str() {
                Some(text) => parts.push(format!(
                    "Updated system prompt section \"{name}\":\n\n{text}"
                )),
                None => parts.push(format!("Removed system prompt section \"{name}\".")),
            }
        }
    }
    parts.join("\n\n")
}

/// TS `createInitialSystemMessage`. Returns `None` when both prompt and tools
/// are empty so an empty transcript stays empty.
pub fn create_initial_system_message(
    system_prompt: Option<&str>,
    tools: Option<&[Tool]>,
) -> Option<Value> {
    let has_prompt = system_prompt.is_some_and(|s| !s.is_empty());
    let has_tools = tools.is_some_and(|t| !t.is_empty());
    if !has_prompt && !has_tools {
        return None;
    }
    let mut obj = serde_json::Map::new();
    obj.insert("role".to_string(), json!("system"));
    obj.insert(
        "content".to_string(),
        json!(system_prompt.unwrap_or_default()),
    );
    if has_tools {
        obj.insert(
            "toolsAdded".to_string(),
            serde_json::to_value(tools.unwrap_or(&[])).unwrap_or(Value::Array(Vec::new())),
        );
    }
    obj.insert("timestamp".to_string(), json!(0));
    Some(Value::Object(obj))
}

/// TS `normalizeContext`: fold `systemPrompt`/`tools` into a leading system
/// message.
pub fn normalize_context(
    system_prompt: Option<&str>,
    tools: Option<&[Tool]>,
    messages: &[Value],
) -> Vec<Value> {
    match create_initial_system_message(system_prompt, tools) {
        Some(initial) => {
            let mut out = Vec::with_capacity(messages.len() + 1);
            out.push(initial);
            out.extend_from_slice(messages);
            out
        }
        None => messages.to_vec(),
    }
}

/// TS `getInitialSystemMessage`: the leading system message, if any.
pub fn get_initial_system_message(messages: &[Value]) -> Option<&Value> {
    messages.first().filter(|m| is_system(m))
}

/// TS `withoutInitialSystemMessage`.
pub fn without_initial_system_message(messages: &[Value]) -> Vec<Value> {
    if get_initial_system_message(messages).is_some() {
        messages[1..].to_vec()
    } else {
        messages.to_vec()
    }
}

fn tools_added(message: &Value) -> Vec<Tool> {
    message
        .get("toolsAdded")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| serde_json::from_value::<Tool>(t.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// TS `getCurrentTools`: apply every system message's tool deltas in order.
pub fn get_current_tools(messages: &[Value]) -> Vec<Tool> {
    let mut order: Vec<String> = Vec::new();
    let mut by_name: HashMap<String, Tool> = HashMap::new();
    for message in messages {
        if !is_system(message) {
            continue;
        }
        if let Some(removed) = message.get("toolsRemoved").and_then(|v| v.as_array()) {
            for r in removed {
                if let Some(name) = r.get("name").and_then(|n| n.as_str()) {
                    by_name.remove(name);
                    order.retain(|n| n != name);
                }
            }
        }
        for tool in tools_added(message) {
            let name = tool.name.clone();
            if !by_name.contains_key(&name) {
                order.push(name.clone());
            }
            by_name.insert(name, tool);
        }
    }
    order.into_iter().filter_map(|n| by_name.remove(&n)).collect()
}

/// Serialize typed messages into the shape the transcript helpers read, or
/// `None` when the transcript carries no system message.
///
/// After P1-7 the declarations live in the transcript's system messages and
/// callers send no `Context.tools` shorthand. `Context::collapsed()` is the
/// exception: for models without mid-conversation system messages it moves the
/// declarations into the shorthand and drops the system messages, so the
/// shorthand is authoritative exactly when no system message is present.
fn transcript_values(messages: &[crate::types::Message]) -> Option<Vec<Value>> {
    if !messages
        .iter()
        .any(|m| matches!(m, crate::types::Message::System { .. }))
    {
        return None;
    }
    serde_json::to_value(messages).ok()?.as_array().cloned()
}

fn shorthand_or_empty(shorthand: Option<&[Tool]>) -> Vec<Tool> {
    shorthand.map_or_else(Vec::new, <[Tool]>::to_vec)
}

/// Tools a request must declare (TS `getCurrentTools(context.messages)`).
#[must_use]
pub fn request_tools(
    messages: &[crate::types::Message],
    shorthand: Option<&[Tool]>,
) -> Vec<Tool> {
    transcript_values(messages).map_or_else(
        || shorthand_or_empty(shorthand),
        |values| get_current_tools(&values),
    )
}

/// Tools the request must declare plus whether `toolsAdded` on system messages
/// may anchor that list (TS `resolveTranscriptTools`).
///
/// When additions anchor the list the request declares only the initial
/// `toolsAdded`; later additions are declared where they happened. Falls back
/// to the shorthand when the transcript carries no system message.
#[must_use]
pub fn transcript_request_tools(
    messages: &[crate::types::Message],
    supports_tool_additions: bool,
    shorthand: Option<&[Tool]>,
) -> (Vec<Tool>, bool) {
    transcript_values(messages).map_or_else(
        || (shorthand_or_empty(shorthand), false),
        |values| resolve_transcript_tools(&values, supports_tool_additions),
    )
}

/// Every tool declared anywhere in the transcript
/// (TS `getDeclaredTools(context.messages)`).
#[must_use]
pub fn declared_tools(
    messages: &[crate::types::Message],
    shorthand: Option<&[Tool]>,
) -> Vec<Tool> {
    transcript_values(messages).map_or_else(
        || shorthand_or_empty(shorthand),
        |values| get_declared_tools(&values),
    )
}

/// TS `getCurrentSystemMessage`: replay all system messages into one.
pub fn get_current_system_message(messages: &[Value]) -> Option<Value> {
    let mut content: Vec<String> = Vec::new();
    let mut sections: Vec<(String, Option<String>)> = Vec::new();
    let mut timestamp: Option<i64> = None;
    for message in messages {
        if !is_system(message) {
            continue;
        }
        if timestamp.is_none() {
            timestamp = message.get("timestamp").and_then(|t| t.as_i64());
        }
        let text = content_text_value(message.get("content").unwrap_or(&Value::Null), "\n");
        if !text.is_empty() {
            content.push(text);
        }
        if let Some(sec) = message.get("sections").and_then(|s| s.as_object()) {
            for (name, value) in sec {
                set_section(&mut sections, name, value.as_str().map(|s| s.to_string()));
            }
        }
    }
    let tools = get_current_tools(messages);
    if timestamp.is_none() && tools.is_empty() {
        return None;
    }
    let mut obj = serde_json::Map::new();
    obj.insert("role".to_string(), json!("system"));
    obj.insert("content".to_string(), json!(content.join("\n\n")));
    let live_sections: Vec<(String, Value)> = sections
        .into_iter()
        .filter_map(|(k, v)| v.map(|v| (k, json!(v))))
        .collect();
    if !live_sections.is_empty() {
        obj.insert("sections".to_string(), Value::Object(live_sections.into_iter().collect()));
    }
    if !tools.is_empty() {
        obj.insert(
            "toolsAdded".to_string(),
            serde_json::to_value(&tools).unwrap_or(Value::Array(Vec::new())),
        );
    }
    obj.insert("timestamp".to_string(), json!(timestamp.unwrap_or(0)));
    Some(Value::Object(obj))
}

/// Insert/update/remove a named section, keeping first-insert position (like
/// JS `Map.set`).
fn set_section(sections: &mut Vec<(String, Option<String>)>, name: &str, value: Option<String>) {
    if let Some(entry) = sections.iter_mut().find(|(n, _)| n == name) {
        entry.1 = value;
    } else {
        sections.push((name.to_string(), value));
    }
}

/// TS `getCurrentSystemPrompt`.
pub fn get_current_system_prompt(messages: &[Value]) -> String {
    get_current_system_message(messages)
        .map(|m| get_system_message_text(&m))
        .unwrap_or_default()
}

/// TS `collapseSystemMessages`: rebuild the leading system message and drop
/// later ones.
pub fn collapse_system_messages(messages: &[Value]) -> Vec<Value> {
    let head = get_current_system_message(messages);
    let mut out: Vec<Value> = Vec::new();
    if let Some(head) = head {
        out.push(head);
    }
    out.extend(messages.iter().filter(|m| !is_system(m)).cloned());
    out
}

/// TS `resolveTranscript`: keep system messages when the model supports them,
/// otherwise collapse.
pub fn resolve_transcript(messages: &[Value], supports_mid_convo_system_messages: Option<bool>) -> Vec<Value> {
    if supports_mid_convo_system_messages.unwrap_or(false) {
        messages.to_vec()
    } else {
        collapse_system_messages(messages)
    }
}

/// TS `toToolDeclaration`: the model-visible declared shape of a tool.
pub fn to_tool_declaration(tool: &Tool) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("name".to_string(), json!(tool.name));
    obj.insert("description".to_string(), json!(tool.description));
    obj.insert("parameters".to_string(), tool.parameters.clone());
    if let Some(cs) = &tool.constrained_sampling {
        obj.insert(
            "constrainedSampling".to_string(),
            serde_json::to_value(cs).unwrap_or(Value::Null),
        );
    }
    Value::Object(obj)
}

/// TS `declarationsEqual`.
pub fn declarations_equal(left: &Tool, right: &Tool) -> bool {
    to_tool_declaration(left) == to_tool_declaration(right)
}

/// TS `getToolStateChanges`: changed definitions count as removal + addition.
pub fn get_tool_state_changes(
    previous: &[Tool],
    current: &[Tool],
) -> (Vec<Tool>, Vec<ToolReference>) {
    let previous_by_name: HashMap<&str, &Tool> =
        previous.iter().map(|t| (t.name.as_str(), t)).collect();
    let current_by_name: HashMap<&str, &Tool> =
        current.iter().map(|t| (t.name.as_str(), t)).collect();
    let added = current
        .iter()
        .filter(|tool| {
            previous_by_name
                .get(tool.name.as_str())
                .map(|prev| !declarations_equal(prev, tool))
                .unwrap_or(true)
        })
        .cloned()
        .collect();
    let removed = previous
        .iter()
        .filter(|tool| {
            current_by_name
                .get(tool.name.as_str())
                .map(|cur| !declarations_equal(tool, cur))
                .unwrap_or(true)
        })
        .map(|tool| ToolReference {
            name: tool.name.clone(),
        })
        .collect();
    (added, removed)
}

/// TS `getDeclaredTools`: every definition referenced by system tool state, in
/// first-declaration order.
pub fn get_declared_tools(messages: &[Value]) -> Vec<Tool> {
    let mut order: Vec<String> = Vec::new();
    let mut by_name: HashMap<String, Tool> = HashMap::new();
    for message in messages {
        if !is_system(message) {
            continue;
        }
        for tool in tools_added(message) {
            let name = tool.name.clone();
            if !by_name.contains_key(&name) {
                order.push(name.clone());
            }
            by_name.insert(name, tool);
        }
    }
    order.into_iter().filter_map(|n| by_name.remove(&n)).collect()
}

/// TS `hasToolRedefinitions`: same name declared twice with different shape.
pub fn has_tool_redefinitions(messages: &[Value]) -> bool {
    let mut declared: HashMap<String, Tool> = HashMap::new();
    for message in messages {
        if !is_system(message) {
            continue;
        }
        for tool in tools_added(message) {
            if let Some(previous) = declared.get(&tool.name) {
                if !declarations_equal(previous, &tool) {
                    return true;
                }
            }
            declared.insert(tool.name.clone(), tool);
        }
    }
    false
}

/// TS `hasNonAdditiveToolChanges`: a removal or same-name redeclaration that an
/// addition-only transport cannot replay.
pub fn has_non_additive_tool_changes(messages: &[Value]) -> bool {
    let mut declared: std::collections::HashSet<String> = std::collections::HashSet::new();
    for message in messages {
        if !is_system(message) {
            continue;
        }
        if message
            .get("toolsRemoved")
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty())
        {
            return true;
        }
        for tool in tools_added(message) {
            if !declared.insert(tool.name.clone()) {
                return true;
            }
        }
    }
    false
}

/// TS `resolveTranscriptTools`: `(requestTools, anchorsAdditions)`.
pub fn resolve_transcript_tools(
    messages: &[Value],
    supports_tool_additions: bool,
) -> (Vec<Tool>, bool) {
    let anchors = supports_tool_additions && !has_non_additive_tool_changes(messages);
    let request_tools = if anchors {
        get_initial_system_message(messages)
            .map(tools_added)
            .unwrap_or_default()
    } else {
        get_current_tools(messages)
    };
    (request_tools, anchors)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::json;

    fn tool(name: &str) -> Tool {
        Tool {
            name: name.to_string(),
            description: String::new(),
            parameters: json!({"type": "object"}),
            constrained_sampling: None,
        }
    }

    fn system(content: &str) -> Value {
        json!({"role": "system", "content": content, "timestamp": 0})
    }

    fn user(text: &str) -> Value {
        json!({"role": "user", "content": text, "timestamp": 0})
    }

    #[test]
    fn normalize_folds_prompt_and_tools() {
        let tools = vec![tool("a")];
        let out = normalize_context(Some("sys"), Some(&tools), &[user("hi")]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["role"], "system");
        assert_eq!(out[0]["content"], "sys");
        assert_eq!(out[0]["toolsAdded"][0]["name"], "a");
        assert_eq!(out[1]["role"], "user");

        // Empty prompt + no tools => unchanged.
        let out = normalize_context(None, None, &[user("hi")]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["role"], "user");
    }

    #[test]
    fn current_tools_apply_removals_and_additions() {
        let messages = vec![
            json!({"role":"system","content":"","toolsAdded":[tool("a"), tool("b")],"timestamp":0}),
            json!({"role":"system","content":"","toolsRemoved":[{"name":"a"}],"toolsAdded":[tool("c")],"timestamp":1}),
        ];
        let names: Vec<String> = get_current_tools(&messages).into_iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["b", "c"]);
    }

    #[test]
    fn current_system_message_replays_content() {
        let messages = vec![system("base"), system("more"), user("hi")];
        let head = get_current_system_message(&messages).unwrap();
        assert_eq!(head["content"], "base\n\nmore");
        assert_eq!(get_current_system_prompt(&messages), "base\n\nmore");
    }

    #[test]
    fn collapse_keeps_one_head() {
        let messages = vec![system("base"), user("hi"), system("more")];
        let out = resolve_transcript(&messages, Some(false));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["role"], "system");
        assert_eq!(out[0]["content"], "base\n\nmore");
        assert_eq!(out[1]["role"], "user");
        // Mid-convo supported keeps the original order.
        let kept = resolve_transcript(&messages, Some(true));
        assert_eq!(kept, messages);
    }

    #[test]
    fn tool_state_changes_treat_redefinition_as_replace() {
        let prev = vec![tool("a"), tool("b")];
        let mut b2 = tool("b");
        b2.description = "changed".into();
        let cur = vec![b2.clone(), tool("c")];
        let (added, removed) = get_tool_state_changes(&prev, &cur);
        // a removed; b changed => removed+added; c added.
        assert_eq!(removed.len(), 2);
        let added_names: Vec<String> = added.into_iter().map(|t| t.name).collect();
        assert!(added_names.contains(&"b".to_string()));
        assert!(added_names.contains(&"c".to_string()));
        assert!(!added_names.contains(&"a".to_string()));
    }

    #[test]
    fn non_additive_detection() {
        let additive = vec![
            json!({"role":"system","content":"","toolsAdded":[tool("a")],"timestamp":0}),
            json!({"role":"system","content":"","toolsAdded":[tool("b")],"timestamp":1}),
        ];
        assert!(!has_non_additive_tool_changes(&additive));
        let (req, anchors) = resolve_transcript_tools(&additive, true);
        assert!(anchors);
        assert_eq!(req.len(), 1); // initial tools only
        assert_eq!(req[0].name, "a");

        let redeclare = vec![
            json!({"role":"system","content":"","toolsAdded":[tool("a")],"timestamp":0}),
            json!({"role":"system","content":"","toolsAdded":[tool("a")],"timestamp":1}),
        ];
        assert!(has_non_additive_tool_changes(&redeclare));
        let (req, anchors) = resolve_transcript_tools(&redeclare, true);
        assert!(!anchors);
        assert_eq!(req.len(), 1);

        let removed = vec![
            json!({"role":"system","content":"","toolsAdded":[tool("a")],"timestamp":0}),
            json!({"role":"system","content":"","toolsRemoved":[{"name":"a"}],"timestamp":1}),
        ];
        assert!(has_non_additive_tool_changes(&removed));
        let (req, _) = resolve_transcript_tools(&removed, true);
        assert!(req.is_empty());
    }

    #[test]
    fn declared_tools_and_redefinitions() {
        let messages = vec![
            json!({"role":"system","content":"","toolsAdded":[tool("a")],"timestamp":0}),
            json!({"role":"system","content":"","toolsAdded":[tool("b")],"timestamp":1}),
        ];
        let names: Vec<String> = get_declared_tools(&messages).into_iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["a", "b"]);
        assert!(!has_tool_redefinitions(&messages));

        let mut a2 = tool("a");
        a2.description = "other".into();
        let redefine = vec![
            json!({"role":"system","content":"","toolsAdded":[tool("a")],"timestamp":0}),
            json!({"role":"system","content":"","toolsAdded":[a2],"timestamp":1}),
        ];
        assert!(has_tool_redefinitions(&redefine));
    }

    #[test]
    fn request_tools_replays_the_transcript() {
        // Declarations live in the transcript's system messages.
        let messages = vec![
            crate::types::Message::System {
                content: crate::types::SystemContent::Text("sys".into()),
                sections: None,
                tools_added: Some(vec![tool("a"), tool("b")]),
                tools_removed: None,
                timestamp: 0,
            },
            crate::types::Message::User {
                content: vec![crate::types::ContentBlock::text("hi")],
                timestamp: 1,
            },
            crate::types::Message::System {
                content: crate::types::SystemContent::Text(String::new()),
                sections: None,
                tools_added: None,
                tools_removed: Some(vec![ToolReference { name: "a".into() }]),
                timestamp: 2,
            },
        ];
        let names: Vec<String> = request_tools(&messages, None)
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, vec!["b"]);
    }

    #[test]
    fn request_tools_uses_the_shorthand_without_system_messages() {
        // `Context::collapsed()` folds the declarations into the shorthand and
        // drops the system messages; the shorthand is then authoritative.
        let shorthand = vec![tool("a")];
        let messages = vec![crate::types::Message::User {
            content: vec![crate::types::ContentBlock::text("hi")],
            timestamp: 0,
        }];
        let names: Vec<String> = request_tools(&messages, Some(&shorthand))
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, vec!["a"]);
        assert!(request_tools(&messages, None).is_empty());
    }
}
