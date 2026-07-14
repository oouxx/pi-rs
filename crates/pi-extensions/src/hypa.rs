//! pi-hypa — Hypa 命令重写扩展。
//!
//! 对应原版 TypeScript 扩展 `pi-hypa.pkg`。
//! 拦截 bash 命令并通过 hypa 二进制重写。

use async_trait::async_trait;
use pi_coding_agent::core::extensions::{
    CommandRegistry, EventResult, ExecResult, ExtensionAPI, ExtensionContext, ExtensionEvent,
    ToolRegistry,
};

pub struct HypaExtension;

impl HypaExtension {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ExtensionAPI for HypaExtension {
    fn name(&self) -> &'static str {
        "pi-hypa"
    }

    fn register_commands(&self, registry: &mut CommandRegistry) {
        registry.register("hypa", Some("Show Hypa Pi extension diagnostics"));
    }

    async fn on_event(&self, _event: &ExtensionEvent, _ctx: &ExtensionContext) -> Option<EventResult> {
        None
    }

    async fn exec(&self, _command: String, _args: Vec<String>) -> Result<ExecResult, String> {
        Err("exec not implemented".into())
    }
}
