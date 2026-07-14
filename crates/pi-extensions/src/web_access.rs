//! pi-web-access — Web 搜索/抓取扩展。
//!
//! 对应原版 TypeScript 扩展 `pi-web-access.pkg`。
//! 提供 web_search、fetch_content、get_search_content 工具。

use async_trait::async_trait;
use pi_coding_agent::core::extensions::{
    CommandRegistry, EventResult, ExtensionAPI, ExtensionContext, ExtensionEvent, ToolCallOutput,
    ToolRegistry,
};

pub struct WebAccessExtension;

impl WebAccessExtension {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ExtensionAPI for WebAccessExtension {
    fn name(&self) -> &'static str {
        "pi-web-access"
    }

    fn register_commands(&self, registry: &mut CommandRegistry) {
        registry.register("websearch", Some("Open web search curator"));
        registry.register("curator", Some("Toggle or configure the search curator workflow"));
        registry.register("google-account", Some("Show the active Google account for Gemini Web"));
        registry.register("search", Some("Browse stored web search results"));
    }

    async fn on_event(&self, _event: &ExtensionEvent, _ctx: &ExtensionContext) -> Option<EventResult> {
        None
    }

    async fn handle_tool_call(
        &self,
        _tool_name: &str,
        _params: serde_json::Value,
        _ctx: &ExtensionContext,
    ) -> Option<ToolCallOutput> {
        None
    }
}