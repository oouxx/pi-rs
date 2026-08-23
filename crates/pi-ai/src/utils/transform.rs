//! 消息预处理层 — 对齐 TS `transform-messages.ts` 的 `transformMessages`。
//!
//! 所有 provider 在 convert 之前调用本层（TS 中 anthropic-messages /
//! openai-completions / openai-responses / google / mistral 等全部先
//! `transformMessages` 再 `convertMessages`）。职责：
//! - **图片降级**（`downgradeUnsupportedImages`）：`model.input` 不含
//!   `"image"` 时，user 消息图片 → `"(image omitted: model does not support
//!   images)"`，toolResult 图片 → `"(tool image omitted: model does not
//!   support images)"`；连续图片只插一个占位（`previousWasPlaceholder` 去重）
//! - **thinking 块处理**：redacted 仅同模型保留；同模型带签名保留；空
//!   thinking 跳过；跨模型转纯文本
//! - **tool call id 归一化**：仅对不同模型（`isSameModel` 门控）的消息
//!   归一化，映射用于匹配 toolResult id
//! - **孤儿 tool call 合成**：assistant 的 tool call 无对应 toolResult 时，
//!   插入 `"No result provided"` 的 isError toolResult（保 thinking 签名、
//!   满足 API 要求）
//! - **跳过 errored/aborted 的 assistant 消息**（不完整 turn 不应回放）

use crate::types::{ContentBlock, Message, Model, StopReason};

const NON_VISION_USER_IMAGE_PLACEHOLDER: &str = "(image omitted: model does not support images)";
const NON_VISION_TOOL_IMAGE_PLACEHOLDER: &str = "(tool image omitted: model does not support images)";

/// 对齐 TS `replaceImagesWithPlaceholder`。
fn replace_images_with_placeholder(
    content: &[ContentBlock],
    placeholder: &str,
) -> Vec<ContentBlock> {
    let mut result: Vec<ContentBlock> = Vec::new();
    let mut previous_was_placeholder = false;
    for block in content {
        match block {
            ContentBlock::Image { .. } => {
                if !previous_was_placeholder {
                    result.push(ContentBlock::text(placeholder));
                }
                previous_was_placeholder = true;
            }
            _ => {
                result.push(block.clone());
                previous_was_placeholder = match block {
                    ContentBlock::Text { text, .. } => text == placeholder,
                    _ => false,
                };
            }
        }
    }
    result
}

/// 对齐 TS `downgradeUnsupportedImages`。
fn downgrade_unsupported_images(messages: &[Message], model: &Model) -> Vec<Message> {
    if model.input.iter().any(|i| i == "image") {
        return messages.to_vec();
    }
    messages
        .iter()
        .map(|msg| match msg {
            Message::User { content, timestamp } => Message::User {
                content: replace_images_with_placeholder(
                    content,
                    NON_VISION_USER_IMAGE_PLACEHOLDER,
                ),
                timestamp: *timestamp,
            },
            Message::ToolResult {
                tool_call_id,
                tool_name,
                content,
                details,
                is_error,
                added_tool_names,
                usage,
                timestamp,
            } => Message::ToolResult {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                content: replace_images_with_placeholder(
                    content,
                    NON_VISION_TOOL_IMAGE_PLACEHOLDER,
                ),
                details: details.clone(),
                is_error: *is_error,
                added_tool_names: added_tool_names.clone(),
                usage: usage.clone(),
                timestamp: *timestamp,
            },
            _ => msg.clone(),
        })
        .collect()
}

/// 对齐 TS `transformMessages`。
///
/// `normalize_tool_call_id` 由各 provider 传入自己的归一化函数
/// （TS 中 `transformMessages(context.messages, model, normalizeToolCallId)`）。
pub fn transform_messages(
    messages: &[Message],
    model: &Model,
    normalize_tool_call_id: &dyn Fn(&str, &Model, &str, &str) -> String,
) -> Vec<Message> {
    // 第一遍：图片降级 + thinking 块处理 + tool call id 归一化。
    let image_aware = downgrade_unsupported_images(messages, model);
    let mut tool_call_id_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    let transformed: Vec<Message> = image_aware
        .iter()
        .map(|msg| match msg {
            // User messages pass through unchanged.
            Message::User { .. } => msg.clone(),
            // toolResult：若 toolCallId 有归一化映射则替换。
            Message::ToolResult {
                tool_call_id,
                tool_name,
                content,
                details,
                is_error,
                added_tool_names,
                usage,
                timestamp,
            } => {
                let normalized_id = tool_call_id_map.get(tool_call_id);
                Message::ToolResult {
                    tool_call_id: normalized_id
                        .cloned()
                        .unwrap_or_else(|| tool_call_id.clone()),
                    tool_name: tool_name.clone(),
                    content: content.clone(),
                    details: details.clone(),
                    is_error: *is_error,
                    added_tool_names: added_tool_names.clone(),
                    usage: usage.clone(),
                    timestamp: *timestamp,
                }
            }
            Message::Assistant {
                content,
                api,
                provider,
                model: msg_model,
                response_model,
                response_id,
                diagnostics,
                usage,
                stop_reason,
                error_message,
                timestamp,
            } => {
                let is_same_model = provider == &model.provider
                    && api == &model.api
                    && msg_model == &model.id;

                let transformed_content: Vec<ContentBlock> = content
                    .iter()
                    .flat_map(|block| match block {
                        ContentBlock::Thinking {
                            thinking,
                            thinking_signature,
                            redacted,
                        } => {
                            // Redacted thinking 是加密内容，仅同模型有效。
                            if redacted.unwrap_or(false) {
                                return if is_same_model {
                                    vec![block.clone()]
                                } else {
                                    Vec::new()
                                };
                            }
                            // 同模型：保留带签名的 thinking（回放需要）。
                            if is_same_model && thinking_signature.is_some() {
                                return vec![block.clone()];
                            }
                            // 跳过空 thinking，其余转纯文本。
                            if thinking.trim().is_empty() {
                                return Vec::new();
                            }
                            if is_same_model {
                                vec![block.clone()]
                            } else {
                                vec![ContentBlock::text(thinking)]
                            }
                        }
                        ContentBlock::Text { text, .. } => {
                            if is_same_model {
                                vec![block.clone()]
                            } else {
                                vec![ContentBlock::text(text)]
                            }
                        }
                        ContentBlock::ToolCall { id, .. } => {
                            let mut normalized: ContentBlock = block.clone();
                            if !is_same_model {
                                // 跨模型丢弃 thoughtSignature。
                                if let ContentBlock::ToolCall {
                                    thought_signature, ..
                                } = &mut normalized
                                {
                                    *thought_signature = None;
                                }
                                let normalized_id =
                                    normalize_tool_call_id(id, model, provider, api);
                                if normalized_id != *id {
                                    tool_call_id_map.insert(id.clone(), normalized_id.clone());
                                    if let ContentBlock::ToolCall { id: nid, .. } =
                                        &mut normalized
                                    {
                                        *nid = normalized_id;
                                    }
                                }
                            }
                            vec![normalized]
                        }
                        _ => vec![block.clone()],
                    })
                    .collect();

                Message::Assistant {
                    content: transformed_content,
                    api: api.clone(),
                    provider: provider.clone(),
                    model: msg_model.clone(),
                    response_model: response_model.clone(),
                    response_id: response_id.clone(),
                    diagnostics: diagnostics.clone(),
                    usage: usage.clone(),
                    stop_reason: stop_reason.clone(),
                    error_message: error_message.clone(),
                    timestamp: *timestamp,
                }
            }
        })
        .collect();

    // 第二遍：孤儿 tool call 合成 + 跳过 errored/aborted assistant。
    let mut result: Vec<Message> = Vec::new();
    let mut pending_tool_calls: Vec<(String, String)> = Vec::new(); // (id, name)
    let mut existing_tool_result_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    let insert_synthetic_tool_results = |result: &mut Vec<Message>,
                                             pending: &mut Vec<(String, String)>,
                                             existing: &mut std::collections::HashSet<String>| {
        if !pending.is_empty() {
            for (id, name) in pending.drain(..) {
                if !existing.contains(&id) {
                    result.push(Message::ToolResult {
                        tool_call_id: id,
                        tool_name: name,
                        content: vec![ContentBlock::text("No result provided")],
                        details: None,
                        is_error: true,
                        added_tool_names: None,
                        usage: None,
                        timestamp: chrono::Utc::now().timestamp_millis(),
                    });
                }
            }
            existing.clear();
        }
    };

    for msg in &transformed {
        match msg {
            Message::Assistant { content, stop_reason, .. } => {
                insert_synthetic_tool_results(
                    &mut result,
                    &mut pending_tool_calls,
                    &mut existing_tool_result_ids,
                );
                // 跳过 errored/aborted 的 assistant 消息（不完整 turn）。
                if matches!(stop_reason, StopReason::Error | StopReason::Aborted) {
                    continue;
                }
                let tool_calls: Vec<(String, String)> = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolCall { id, name, .. } => {
                            Some((id.clone(), name.clone()))
                        }
                        _ => None,
                    })
                    .collect();
                if !tool_calls.is_empty() {
                    pending_tool_calls = tool_calls;
                    existing_tool_result_ids.clear();
                }
                result.push(msg.clone());
            }
            Message::ToolResult { tool_call_id, .. } => {
                existing_tool_result_ids.insert(tool_call_id.clone());
                result.push(msg.clone());
            }
            Message::User { .. } => {
                // user 消息打断 tool 流：先合成孤儿结果。
                insert_synthetic_tool_results(
                    &mut result,
                    &mut pending_tool_calls,
                    &mut existing_tool_result_ids,
                );
                result.push(msg.clone());
            }
        }
    }
    insert_synthetic_tool_results(
        &mut result,
        &mut pending_tool_calls,
        &mut existing_tool_result_ids,
    );

    result
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::types::{ModelCost, Usage};

    fn model_with_input(input: Vec<&str>) -> Model {
        Model {
            id: "test-model".into(),
            name: "Test".into(),
            api: "openai-completions".into(),
            provider: "test".into(),
            base_url: "https://example.com/v1".into(),
            reasoning: false,
            thinking_level_map: None,
            input: input.into_iter().map(str::to_string).collect(),
            cost: ModelCost::default(),
            context_window: 128000,
            max_tokens: 4096,
            sampling_params: None,
            headers: None,
            compat: None,
        }
    }

    fn user_msg(content: Vec<ContentBlock>) -> Message {
        Message::User {
            content,
            timestamp: 0,
        }
    }

    fn assistant_msg(content: Vec<ContentBlock>, stop: StopReason) -> Message {
        Message::Assistant {
            content,
            api: "openai-completions".into(),
            provider: "test".into(),
            model: "test-model".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: stop,
            error_message: None,
            timestamp: 0,
        }
    }

    fn noop_normalize(id: &str, _m: &Model, _p: &str, _a: &str) -> String {
        id.to_string()
    }

    #[test]
    fn test_vision_model_images_pass_through() {
        let model = model_with_input(vec!["text", "image"]);
        let img = ContentBlock::Image {
            data: "AAAA".into(),
            mime_type: "image/png".into(),
        };
        let msgs = vec![user_msg(vec![ContentBlock::text("hi"), img.clone()])];
        let out = transform_messages(&msgs, &model, &noop_normalize);
        assert!(matches!(
            &out[0],
            Message::User { content, .. } if content.len() == 2
        ));
    }

    #[test]
    fn test_non_vision_user_image_downgraded() {
        let model = model_with_input(vec!["text"]);
        let img = ContentBlock::Image {
            data: "AAAA".into(),
            mime_type: "image/png".into(),
        };
        let msgs = vec![user_msg(vec![ContentBlock::text("hi"), img])];
        let out = transform_messages(&msgs, &model, &noop_normalize);
        if let Message::User { content, .. } = &out[0] {
            assert_eq!(content.len(), 2);
            assert_eq!(
                content[1],
                ContentBlock::text("(image omitted: model does not support images)")
            );
        } else {
            panic!("expected user");
        }
    }

    #[test]
    fn test_non_vision_consecutive_images_single_placeholder() {
        let model = model_with_input(vec!["text"]);
        let img = || ContentBlock::Image {
            data: "AAAA".into(),
            mime_type: "image/png".into(),
        };
        let msgs = vec![user_msg(vec![img(), img(), ContentBlock::text("x")])];
        let out = transform_messages(&msgs, &model, &noop_normalize);
        if let Message::User { content, .. } = &out[0] {
            // 两个连续图片 → 一个占位 + 文本
            let placeholders = content
                .iter()
                .filter(|b| {
                    matches!(b, ContentBlock::Text { text, .. }
                        if text == "(image omitted: model does not support images)")
                })
                .count();
            assert_eq!(placeholders, 1);
        } else {
            panic!("expected user");
        }
    }

    #[test]
    fn test_non_vision_tool_result_image_downgraded() {
        let model = model_with_input(vec!["text"]);
        let img = ContentBlock::Image {
            data: "AAAA".into(),
            mime_type: "image/png".into(),
        };
        let msgs = vec![Message::ToolResult {
            tool_call_id: "call_1".into(),
            tool_name: "read".into(),
            content: vec![img],
            details: None,
            is_error: false,
            added_tool_names: None,
            usage: None,
            timestamp: 0,
        }];
        let out = transform_messages(&msgs, &model, &noop_normalize);
        if let Message::ToolResult { content, .. } = &out[0] {
            assert_eq!(
                content[0],
                ContentBlock::text("(tool image omitted: model does not support images)")
            );
        } else {
            panic!("expected toolResult");
        }
    }

    #[test]
    fn test_orphan_tool_call_gets_synthetic_result() {
        let model = model_with_input(vec!["text"]);
        let msgs = vec![assistant_msg(
            vec![ContentBlock::ToolCall {
                id: "call_1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({}),
                thought_signature: None,
                namespace: None,
            }],
            StopReason::Stop,
        )];
        let out = transform_messages(&msgs, &model, &noop_normalize);
        assert_eq!(out.len(), 2, "assistant + synthetic toolResult");
        if let Message::ToolResult { content, is_error, .. } = &out[1] {
            assert!(*is_error);
            assert_eq!(content[0], ContentBlock::text("No result provided"));
        } else {
            panic!("expected synthetic toolResult");
        }
    }

    #[test]
    fn test_errored_assistant_skipped() {
        let model = model_with_input(vec!["text"]);
        let msgs = vec![assistant_msg(
            vec![ContentBlock::text("partial")],
            StopReason::Error,
        )];
        let out = transform_messages(&msgs, &model, &noop_normalize);
        assert!(out.is_empty(), "errored assistant must be skipped");
    }

    #[test]
    fn test_cross_model_tool_call_id_normalized() {
        let model = model_with_input(vec!["text"]);
        // 跨模型：assistant 消息来自不同的 provider/model。
        let foreign = Message::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: "call_1|item_2".into(),
                name: "bash".into(),
                arguments: serde_json::json!({}),
                thought_signature: None,
                namespace: None,
            }],
            api: "openai-responses".into(),
            provider: "other-provider".into(),
            model: "other-model".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        };
        let msgs = vec![
            foreign,
            Message::ToolResult {
                tool_call_id: "call_1|item_2".into(),
                tool_name: "bash".into(),
                content: vec![ContentBlock::text("out")],
                details: None,
                is_error: false,
                added_tool_names: None,
                usage: None,
                timestamp: 0,
            },
        ];
        let normalize = |id: &str, _m: &Model, _p: &str, _a: &str| {
            if id.contains('|') {
                "norm_call|fc_norm_item".to_string()
            } else {
                id.to_string()
            }
        };
        let out = transform_messages(&msgs, &model, &normalize);
        // assistant 的 toolCall id 归一化
        if let Message::Assistant { content, .. } = &out[0] {
            if let ContentBlock::ToolCall { id, .. } = &content[0] {
                assert_eq!(id, "norm_call|fc_norm_item");
            }
        }
        // toolResult 的 id 跟随映射
        if let Message::ToolResult { tool_call_id, .. } = &out[1] {
            assert_eq!(tool_call_id, "norm_call|fc_norm_item");
        }
    }

    #[test]
    fn test_same_model_tool_call_id_unchanged() {
        let model = model_with_input(vec!["text"]);
        let msgs = vec![assistant_msg(
            vec![ContentBlock::ToolCall {
                id: "call_1|item_2".into(),
                name: "bash".into(),
                arguments: serde_json::json!({}),
                thought_signature: None,
                namespace: None,
            }],
            StopReason::Stop,
        )];
        let normalize = |id: &str, _m: &Model, _p: &str, _a: &str| {
            format!("NORM_{id}") // 同模型不应被调用
        };
        let out = transform_messages(&msgs, &model, &normalize);
        if let Message::Assistant { content, .. } = &out[0] {
            if let ContentBlock::ToolCall { id, .. } = &content[0] {
                assert_eq!(id, "call_1|item_2", "same-model ids must stay raw");
            }
        }
    }
}
