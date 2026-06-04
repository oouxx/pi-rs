//! Structured data extraction from LLM prompts.
//!
//! Wraps a target type's JSON Schema as a tool/function call. The model "calls"
//! the extraction tool, and the tool call arguments **are** the extracted data —
//! no text post-processing needed.
//!
//! Inspired by [Rig's `Extractor`](https://docs.rs/rig-core/latest/rig_core/extractor/index.html):
//! Rig wraps the target struct as a `SubmitTool<T>` that implements the `Tool` trait
//! with `Args = T`, so the model's tool call arguments are automatically deserialized
//! as `T` by the framework. Our version achieves the same result with less
//! infrastructure: we build a `Context` with the tool inline, call
//! [`pi_ai::stream::complete`], and parse the tool call arguments manually.
//!
//! # Required providers
//!
//! Call [`pi_ai::providers::register_builtins::register_built_in_api_providers`]
//! before using any extraction.
//!
//! # Example
//!
//! ```rust,ignore
//! use pi_agent_core::extraction::{Extractor, JsonSchema};
//! use serde::Deserialize;
//!
//! #[derive(Deserialize, JsonSchema)]
//! struct Person {
//!     name: String,
//!     age: u8,
//!     city: String,
//! }
//!
//! let model = pi_ai::types::Model {
//!     id: "deepseek-v4-flash".into(),
//!     api: "openai-completions".into(),
//!     provider: "deepseek".into(),
//!     base_url: "https://api.deepseek.com".into(),
//!     ..Default::default()
//! };
//!
//! let extractor: Extractor<Person> = Extractor::new(model)
//!     .with_api_key(std::env::var("DEEPSEEK_API_KEY").unwrap())
//!     .with_retries(2)
//!     .with_system_prompt("Extract person information from the user message.");
//!
//! let person = extractor.extract("John is 30 and lives in NY.").await.unwrap();
//! ```

use std::marker::PhantomData;

pub use schemars::JsonSchema;
use schemars::schema_for;
use serde::de::DeserializeOwned;

use pi_ai::stream;
use pi_ai::types::{
    self, ContentBlock, Context, Message, Model, StreamOptions, Tool,
};

// ============================================================================
// Error type
// ============================================================================

/// Errors that can occur during extraction.
///
/// Mirrors Rig's [`ExtractionError`].
#[derive(Debug)]
pub enum ExtractError {
    /// The LLM call itself failed.
    Llm(String),
    /// No tool call was found for the extraction tool.
    NoToolCall(String),
    /// Multiple tool calls were found for the extraction tool.
    MultipleToolCalls(String),
    /// Failed to parse tool call arguments as the target type.
    Deserialize(String),
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Llm(msg) => write!(f, "LLM error: {msg}"),
            Self::NoToolCall(name) => write!(f, "no tool call found for '{name}'"),
            Self::MultipleToolCalls(name) => write!(f, "multiple tool calls found for '{name}'"),
            Self::Deserialize(msg) => write!(f, "deserialization error: {msg}"),
        }
    }
}

impl std::error::Error for ExtractError {}

// ============================================================================
// Extraction response
// ============================================================================

/// Response from an extraction operation, containing the extracted data
/// and accumulated token usage (across all retries).
#[derive(Debug)]
pub struct ExtractionResponse<T> {
    /// The extracted structured data.
    pub data: T,
    /// Accumulated token usage across all attempts.
    pub usage: types::Usage,
}

// ============================================================================
// Extractor
// ============================================================================

/// Extracts structured data from user prompts by wrapping the target type's
/// JSON Schema as a tool call.
///
/// The model "calls" the extraction tool, and the tool call arguments **are**
/// the extracted data — no text post-processing needed.
///
/// Inspired by Rig's `Extractor<M, T>` which uses a `SubmitTool<T>` internally
/// (a `Tool` impl with `Args = T`). Our version builds the tool context inline
/// and calls [`pi_ai::stream::complete`] directly, which avoids the need for
/// the full [`Agent`](crate::agent::Agent) infrastructure.
pub struct Extractor<T> {
    model: Model,
    api_key: Option<String>,
    tool_name: String,
    tool_description: String,
    system_prompt: String,
    retries: u32,
    _phantom: PhantomData<T>,
}

impl<T: JsonSchema + DeserializeOwned> Extractor<T> {
    /// Create a new `Extractor`.
    ///
    /// The tool name defaults to the type name (from `schemars::JsonSchema`).
    /// Call [`with_api_key`](Self::with_api_key) to set the API key.
    pub fn new(model: Model) -> Self {
        let schema = schema_for!(T);
        let tool_name = schema
            .schema
            .metadata
            .as_ref()
            .and_then(|m| m.title.as_deref())
            .unwrap_or("extracted_data")
            .to_string();

        Self {
            model,
            api_key: None,
            tool_name,
            tool_description: String::new(),
            system_prompt: String::new(),
            retries: 0,
            _phantom: PhantomData,
        }
    }

    /// Set the API key used to call the LLM.
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Override the default tool name.
    ///
    /// Corresponds to setting a custom tool name in Rig's `SubmitTool`.
    pub fn with_tool_name(mut self, name: impl Into<String>) -> Self {
        self.tool_name = name.into();
        self
    }

    /// Set the tool description sent to the LLM.
    ///
    /// Corresponds to Rig's `ToolDefinition.description`.
    pub fn with_tool_description(mut self, desc: impl Into<String>) -> Self {
        self.tool_description = desc.into();
        self
    }

    /// Set the system prompt sent with every extraction request.
    ///
    /// Equivalent to Rig's [`ExtractorBuilder::preamble`].
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Set the number of retries on extraction failure.
    ///
    /// The extractor retries when the model does not call the extraction tool
    /// or returns unparsable data.  Default is `0` (no retry).
    ///
    /// Equivalent to Rig's `retries` field on `Extractor`.
    pub fn with_retries(mut self, retries: u32) -> Self {
        self.retries = retries;
        self
    }

    // ------------------------------------------------------------------
    // Public extraction methods
    // ------------------------------------------------------------------

    /// Extract structured data from the given prompt text.
    ///
    /// Retries up to [`self.retries`](Self::with_retries) times on failure.
    ///
    /// Equivalent to Rig's [`Extractor::extract`].
    pub async fn extract(&self, text: &str) -> Result<T, ExtractError> {
        self.extract_with_chat_history(text, &[]).await
    }

    /// Extract structured data with additional chat history context.
    ///
    /// The `chat_history` messages are prepended to the user's prompt so the
    /// model has conversational context.
    ///
    /// Equivalent to Rig's [`Extractor::extract_with_chat_history`].
    pub async fn extract_with_chat_history(
        &self,
        text: &str,
        chat_history: &[Message],
    ) -> Result<T, ExtractError> {
        self.extract_inner(text, chat_history).await
    }

    /// Extract structured data and return it together with accumulated usage.
    ///
    /// Equivalent to Rig's [`Extractor::extract_with_usage`].
    pub async fn extract_with_usage(
        &self,
        text: &str,
    ) -> Result<ExtractionResponse<T>, ExtractError> {
        self.extract_with_chat_history_with_usage(text, &[]).await
    }

    /// Full extraction: with chat history, returns data + accumulated usage.
    ///
    /// Equivalent to Rig's [`Extractor::extract_with_chat_history_with_usage`].
    pub async fn extract_with_chat_history_with_usage(
        &self,
        text: &str,
        chat_history: &[Message],
    ) -> Result<ExtractionResponse<T>, ExtractError> {
        // Directly calls the usage-tracking inner loop instead of chaining
        // extract_inner → extract_with_usage_inner (which would make TWO
        // LLM calls). Rig merges chat_history + retries + usage tracking
        // into a single `extract_json_with_usage` method; we do the same.
        self.extract_with_usage_inner(text, chat_history).await
    }

    // ------------------------------------------------------------------
    // Internal implementations
    // ------------------------------------------------------------------

    /// Build the tool definition from T's JSON Schema.
    fn build_tool(&self) -> Tool {
        let schema = schema_for!(T);
        let schema_value = serde_json::to_value(&schema)
            .expect("schemars schema should always serialize");

        Tool {
            name: self.tool_name.clone(),
            description: if self.tool_description.is_empty() {
                format!(
                    "Extract structured data matching the JSON schema: {}",
                    schema_value
                )
            } else {
                self.tool_description.clone()
            },
            parameters: schema_value,
        }
    }

    /// Build the base context (system prompt + chat history + user text).
    fn build_context(
        &self,
        text: &str,
        chat_history: &[Message],
        tool: Tool,
        reinforce: bool,
    ) -> Context {
        let mut messages = chat_history.to_vec();

        if reinforce {
            // On retry: add a reinforced instruction so the model knows it
            // must call the tool (Rig's retry does the same — see
            // Extractor::extract_json_with_usage).
            messages.push(Message::User {
                content: vec![ContentBlock::Text {
                    text: format!(
                        "You MUST call the '{}' tool. \
                         Do NOT respond with text — only call the tool with the extracted data.",
                        self.tool_name
                    ),
                    text_signature: None,
                }],
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
        }

        messages.push(Message::User {
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                text_signature: None,
            }],
            timestamp: chrono::Utc::now().timestamp_millis(),
        });

        Context {
            system_prompt: if self.system_prompt.is_empty() {
                None
            } else {
                Some(self.system_prompt.clone())
            },
            messages,
            tools: Some(vec![tool]),
        }
    }

    /// Build stream options (api_key, no tool_choice — see note below).
    fn build_options(&self) -> StreamOptions {
        StreamOptions {
            api_key: self.api_key.clone(),
            // Note: we intentionally do NOT set tool_choice here.
            // Many reasoning-model APIs (e.g. DeepSeek V4 Flash) auto-enable
            // thinking mode and reject any explicit tool_choice with
            // "thinking mode does not support this tool_choice".
            // With only one tool available, the model naturally calls it
            // when the system prompt instructs extraction.
            tool_choice: None,
            ..Default::default()
        }
    }

    /// Parse tool call arguments from an `AssistantMessage`.
    fn parse_tool_call(&self, msg: &types::AssistantMessage) -> Result<T, ExtractError> {
        let mut matches: Vec<serde_json::Value> = Vec::new();
        for block in &msg.content {
            if let ContentBlock::ToolCall { name, arguments, .. } = block {
                if name == &self.tool_name {
                    matches.push(arguments.clone());
                }
            }
        }

        match matches.len() {
            0 => Err(ExtractError::NoToolCall(self.tool_name.clone())),
            1 => serde_json::from_value(matches.remove(0))
                .map_err(|e| ExtractError::Deserialize(e.to_string())),
            _ => Err(ExtractError::MultipleToolCalls(self.tool_name.clone())),
        }
    }

    /// Core extraction loop (without usage tracking).
    async fn extract_inner(
        &self,
        text: &str,
        chat_history: &[Message],
    ) -> Result<T, ExtractError> {
        let tool = self.build_tool();
        let max_attempts = 1 + self.retries;

        for attempt in 0..max_attempts {
            let reinforce = attempt > 0;
            let context = self.build_context(text, chat_history, tool.clone(), reinforce);
            let options = self.build_options();

            let result = stream::complete(&self.model, &context, Some(options))
                .await
                .map_err(|e| ExtractError::Llm(e))?;

            match self.parse_tool_call(&result) {
                Ok(data) => return Ok(data),
                Err(ExtractError::NoToolCall(_)) if attempt + 1 < max_attempts => {
                    // Retry — next iteration adds a reinforced instruction
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        Err(ExtractError::NoToolCall(self.tool_name.clone()))
    }

    /// Core extraction loop with usage tracking (returns `ExtractionResponse<T>`).
    async fn extract_with_usage_inner(
        &self,
        text: &str,
        chat_history: &[Message],
    ) -> Result<ExtractionResponse<T>, ExtractError> {
        let tool = self.build_tool();
        let max_attempts = 1 + self.retries;
        let mut accumulated_usage = types::Usage::default();

        for attempt in 0..max_attempts {
            let reinforce = attempt > 0;
            let context = self.build_context(text, chat_history, tool.clone(), reinforce);
            let options = self.build_options();

            let result = stream::complete(&self.model, &context, Some(options))
                .await
                .map_err(|e| ExtractError::Llm(e))?;

            // Accumulate usage (input/output/cache tokens across retries)
            accumulated_usage.input += result.usage.input;
            accumulated_usage.output += result.usage.output;
            accumulated_usage.cache_read += result.usage.cache_read;
            accumulated_usage.cache_write += result.usage.cache_write;
            accumulated_usage.total_tokens += result.usage.total_tokens;

            match self.parse_tool_call(&result) {
                Ok(data) => {
                    return Ok(ExtractionResponse {
                        data,
                        usage: accumulated_usage,
                    });
                }
                Err(ExtractError::NoToolCall(_)) if attempt + 1 < max_attempts => {
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        Err(ExtractError::NoToolCall(self.tool_name.clone()))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    #[derive(JsonSchema, serde::Deserialize, Debug, PartialEq)]
    struct TestPerson {
        name: String,
        age: u8,
        city: String,
    }

    fn dummy_model() -> Model {
        Model {
            id: "test".into(),
            name: String::new(),
            api: "openai-completions".into(),
            provider: "test".into(),
            base_url: String::new(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: types::ModelCost::default(),
            context_window: 0,
            max_tokens: 0,
            headers: None,
            compat: None,
        }
    }

    #[test]
    fn test_extractor_default_tool_name_from_schema() {
        let extractor = Extractor::<TestPerson>::new(dummy_model());
        assert_eq!(extractor.tool_name, "TestPerson");
    }

    #[test]
    fn test_extractor_custom_tool_name() {
        let extractor = Extractor::<TestPerson>::new(dummy_model())
            .with_tool_name("extract_person");
        assert_eq!(extractor.tool_name, "extract_person");
    }

    #[test]
    fn test_extractor_custom_system_prompt() {
        let extractor = Extractor::<TestPerson>::new(dummy_model())
            .with_system_prompt("Extract person info.");
        assert_eq!(extractor.system_prompt, "Extract person info.");
    }

    #[test]
    fn test_extractor_retries_default() {
        let extractor = Extractor::<TestPerson>::new(dummy_model());
        assert_eq!(extractor.retries, 0);
    }

    #[test]
    fn test_extractor_custom_retries() {
        let extractor = Extractor::<TestPerson>::new(dummy_model())
            .with_retries(3);
        assert_eq!(extractor.retries, 3);
    }

    #[test]
    fn test_extractor_errors_display() {
        let err = ExtractError::NoToolCall("test".into());
        assert_eq!(format!("{err}"), "no tool call found for 'test'");

        let err = ExtractError::Llm("API error".into());
        assert_eq!(format!("{err}"), "LLM error: API error");

        let err = ExtractError::Deserialize("bad json".into());
        assert_eq!(format!("{err}"), "deserialization error: bad json");

        let err = ExtractError::MultipleToolCalls("test".into());
        assert_eq!(format!("{err}"), "multiple tool calls found for 'test'");
    }

    #[test]
    fn test_extraction_response_debug() {
        let response = ExtractionResponse {
            data: TestPerson {
                name: "Alice".into(),
                age: 25,
                city: "Paris".into(),
            },
            usage: types::Usage::default(),
        };
        let debug = format!("{response:?}");
        assert!(debug.contains("Alice"));
    }

    #[test]
    fn test_build_tool_has_name_and_params() {
        let extractor = Extractor::<TestPerson>::new(dummy_model())
            .with_tool_name("extract_person");
        let tool = extractor.build_tool();
        assert_eq!(tool.name, "extract_person");
        assert!(!tool.description.is_empty());
        // Should contain the JSON schema
        let schema_str = serde_json::to_string(&tool.parameters).unwrap();
        assert!(schema_str.contains("name"));
        assert!(schema_str.contains("age"));
        assert!(schema_str.contains("city"));
    }

    #[test]
    fn test_build_context_includes_messages() {
        let extractor = Extractor::<TestPerson>::new(dummy_model());
        let tool = extractor.build_tool();
        let context = extractor.build_context("Hello", &[], tool, false);
        assert_eq!(context.messages.len(), 1);
    }

    #[test]
    fn test_build_context_with_reinforce_adds_extra_message() {
        let extractor = Extractor::<TestPerson>::new(dummy_model());
        let tool = extractor.build_tool();
        let context = extractor.build_context("Hello", &[], tool, true);
        assert_eq!(context.messages.len(), 2);
        // First message should be the reinforce instruction
        if let Message::User { content, .. } = &context.messages[0] {
            assert!(content.iter().any(|b| matches!(b, ContentBlock::Text { text, .. }
                if text.contains("MUST"))));
        } else {
            panic!("Expected User message");
        }
    }

    #[test]
    fn test_build_context_with_chat_history() {
        let extractor = Extractor::<TestPerson>::new(dummy_model());
        let tool = extractor.build_tool();
        let history = vec![
            Message::User {
                content: vec![ContentBlock::Text {
                    text: "Previous message".into(),
                    text_signature: None,
                }],
                timestamp: 0,
            },
        ];
        let context = extractor.build_context("Hello", &history, tool, false);
        assert_eq!(context.messages.len(), 2);
    }

    #[test]
    fn test_extractor_api_key_is_set() {
        let extractor = Extractor::<TestPerson>::new(dummy_model())
            .with_api_key("sk-test");
        assert_eq!(extractor.api_key, Some("sk-test".to_string()));
    }
}
