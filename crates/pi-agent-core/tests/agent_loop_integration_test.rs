use pi_agent_core::agent::{create_agent, Agent, AgentOptions};
use pi_agent_core::pi_ai_types::{text_block, ContentBlock, Model};
use pi_agent_core::types::{AgentEvent, AgentMessage, AgentState};
use pi_agent_core::types::{ ConvertToLlmFn, DynTool, StreamFn, StreamFnOptions};
use pi_ai::models::get_model;
use pi_ai::providers::register_builtins::register_built_in_api_providers;
use std::sync::Arc;
#[tokio::test]
async fn test_agent_process() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    register_built_in_api_providers();

    let model = get_model("deepseek", "deepseek-v4-flash").expect("Model not found");


    // 1. 定义 StreamFn（LLM 驱动）
    let stream_fn: StreamFn = Arc::new(|model, context, thinking_level, options| {
        Box::pin(async move {
            // 调用 LLM API，返回 Stream<AssistantMessageEvent>
            // 参见 pi-ai 的 stream::stream_request()
            todo!("实现流式 LLM 调用")
        })
    });

    // 2. 定义消息转换函数
    let convert_to_llm: ConvertToLlmFn =
        Arc::new(|messages| pi_agent_core::harness::messages::convert_to_llm(messages));

    // 3. 创建 Agent（使用 create_agent 便捷函数）
    let agent = create_agent(
        model.clone(),                          // pi_ai::types::Model
        "You are a helpful assistant.", // system prompt
        vec![],                         // 工具列表
        stream_fn,
        convert_to_llm,
    );

    // 4. 发送消息
    use chrono::Utc;
    let msg = AgentMessage::User {
        content: vec![text_block("Hello!")],
        timestamp: Utc::now().timestamp_millis(),
    };
    let results = agent.process(vec![msg]).await?;

    // 5. 读取回复
    for msg in &results {
        if let AgentMessage::Assistant {
            content,
            model,
            usage,
            ..
        } = msg
        {
            for block in content {
                match block {
                    ContentBlock::Text { text, .. } => println!("{}", text),
                    ContentBlock::Thinking { thinking, .. } => println!("[thinking] {}", thinking),
                    _ => {}
                }
            }
        }
    }
    Ok(())
}
