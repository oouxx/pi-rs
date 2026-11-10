//! Runtime tool registration for tools that appear after session start.
//!
//! MCP servers may announce `notifications/tools/list_changed`; the session
//! cannot rebuild, so this manager owns the mutable side of the tool registry:
//! registered definitions, their `AgentTool` wrappers, and the `tool_search`
//! index. `AgentSession` consults it alongside its immutable startup registry.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use pi_agent_core::agent::Agent;
use pi_agent_core::types::{AgentTool, DynTool};
use pi_extension_api::{ToolDefinition, ToolExposure};
use serde_json::{json, Value};

use crate::core::tool_search::{create_tool_search_document, Bm25Ranker, ToolSearchDocument};

#[derive(Default)]
struct SearchIndex {
    documents: Vec<ToolSearchDocument>,
    registry: HashMap<String, Arc<DynTool>>,
}

/// Mutable registry for tools added after session construction.
#[derive(Default)]
pub struct RuntimeToolManager {
    agent: Mutex<Option<Arc<Agent>>>,
    registry: RwLock<Vec<Arc<DynTool>>>,
    definitions: RwLock<HashMap<String, ToolDefinition>>,
    search: RwLock<SearchIndex>,
    registered: AtomicBool,
    tool_search_present: AtomicBool,
}

impl std::fmt::Debug for RuntimeToolManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeToolManager")
            .field("tools", &self.definitions.read().map(|defs| defs.len()).unwrap_or(0))
            .finish()
    }
}

fn searchable(exposure: ToolExposure) -> bool {
    matches!(exposure, ToolExposure::Deferred | ToolExposure::Codemode)
}

fn direct(exposure: ToolExposure) -> bool {
    matches!(exposure, ToolExposure::Direct | ToolExposure::ModelOnly)
}

#[allow(clippy::type_complexity)]
type AgentToolExecuteFn = Arc<
    dyn Fn(
            String,
            Value,
            Option<tokio::sync::watch::Receiver<bool>>,
            Option<
                Arc<
                    dyn Fn(pi_agent_core::types::AgentToolResult<Value>) + Send + Sync,
                >,
            >,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            pi_agent_core::types::AgentToolResult<Value>,
                            Box<dyn std::error::Error + Send + Sync>,
                        >,
                    > + Send,
            >,
        > + Send
        + Sync,
>;

type ToolSearchFuture = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<
                    pi_agent_core::types::AgentToolResult<Value>,
                    Box<dyn std::error::Error + Send + Sync>,
                >,
            > + Send,
    >,
>;

fn agent_tool_from_definition(definition: &ToolDefinition) -> DynTool {
    let name = definition.name.clone();
    let execute: AgentToolExecuteFn = match &definition.execute {
        Some(exec) => {
            let exec = Arc::clone(exec);
            Arc::new(move |id, params, signal, _on_update| {
                let exec = Arc::clone(&exec);
                Box::pin(async move {
                    let output = exec(id, params, signal).await?;
                    crate::core::tools::tool_definition_wrapper::tool_call_output_to_agent_result(
                        output,
                    )
                })
            })
        }
        None => {
            let name = name.clone();
            Arc::new(move |_id, _params, _signal, _on_update| {
                let error: Box<dyn std::error::Error + Send + Sync> =
                    format!("Tool '{name}' has no execute implementation").into();
                Box::pin(async move { Err(error) })
            })
        }
    };
    AgentTool {
        constrained_sampling: definition.constrained_sampling.clone(),
        name: definition.name.clone(),
        description: definition.description.clone(),
        label: definition.label.clone().unwrap_or_default(),
        prompt_snippet: definition.prompt_snippet.clone(),
        prompt_guidelines: definition.prompt_guidelines.clone(),
        parameters_schema: definition
            .parameters
            .clone()
            .unwrap_or_else(|| json!({"type": "object", "properties": {}, "required": []})),
        execution_mode: definition.execution_mode.as_deref().and_then(|mode| match mode {
            "sequential" => Some(pi_agent_core::pi_ai_types::ToolExecutionMode::Sequential),
            "parallel" => Some(pi_agent_core::pi_ai_types::ToolExecutionMode::Parallel),
            _ => None,
        }),
        prepare_arguments: None,
        execute,
    }
}

impl RuntimeToolManager {
    /// Bind the agent that receives direct tools.
    pub fn set_agent(&self, agent: Arc<Agent>) {
        *self.agent.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(agent);
    }

    /// Mark that `tool_search` was registered by the session's initial setup.
    pub fn mark_tool_search_present(&self) {
        self.tool_search_present.store(true, Ordering::SeqCst);
    }

    /// Register `tool_search` on demand for tools that appear after startup.
    pub async fn ensure_tool_search(self: &Arc<Self>) {
        if self.tool_search_present.swap(true, Ordering::SeqCst) {
            return;
        }
        let manager = Arc::clone(self);
        let execute: AgentToolExecuteFn = Arc::new(
            move |_tool_call_id: String,
                  params: Value,
                  _signal: Option<tokio::sync::watch::Receiver<bool>>,
                  _on_update: Option<
                Arc<dyn Fn(pi_agent_core::types::AgentToolResult<Value>) + Send + Sync>,
            >| {
                let manager = Arc::clone(&manager);
                let future: ToolSearchFuture = Box::pin(async move {
                    let query = params
                        .get("query")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "query must be a string".to_string())?
                        .to_string();
                    if query.trim().is_empty() {
                        return Err("query must not be empty".to_string().into());
                    }
                    let limit = params
                        .get("limit")
                        .and_then(Value::as_u64)
                        .map(|limit| limit as usize)
                        .unwrap_or(8);
                    if limit == 0 {
                        return Err("limit must be a positive integer".to_string().into());
                    }
                    let agent = manager
                        .agent
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone()
                        .ok_or_else(|| "tool_search is not ready".to_string())?;
                    let state = agent.state().await;
                    let active: HashSet<String> =
                        state.tools.iter().map(|tool| tool.name.clone()).collect();
                    let matches = manager.search(&query, &active, limit);
                    let additions: Vec<Arc<DynTool>> =
                        matches.iter().map(|(_, tool)| Arc::clone(tool)).collect();
                    let names: Vec<String> =
                        matches.iter().map(|(name, _)| name.clone()).collect();
                    if additions.is_empty() {
                        return Ok(pi_agent_core::types::AgentToolResult {
                            content: vec![pi_agent_core::pi_ai_types::text_block(
                                "No matching tools found.".to_string(),
                            )],
                            details: json!({"loaded": []}),
                            usage: None,
                            terminate: None,
                            structured_content: None,
                        });
                    }
                    agent.add_tools(additions).await;
                    let text = format!(
                        "Loaded {} tool{}. They are available from your next call:
{}",
                        names.len(),
                        if names.len() == 1 { "" } else { "s" },
                        names
                            .iter()
                            .map(|name| format!("- {name}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    );
                    Ok(pi_agent_core::types::AgentToolResult {
                        content: vec![pi_agent_core::pi_ai_types::text_block(text)],
                        details: json!({"loaded": names}),
                        usage: None,
                        terminate: None,
                        structured_content: None,
                    })
                });
                future
            },
        );
        let definition = ToolDefinition {
            name: "tool_search".to_string(),
            label: Some("tool_search".to_string()),
            description: "# Tool discovery\n\nSearches over deferred tool metadata with BM25 and exposes matching tools for the next model call.\n\nSome of the tools, such as tools of MCP servers, may not have been provided to you upfront, and you should use this tool (`tool_search`) to search for the required tools. For MCP tool discovery, always use `tool_search`.".to_string(),
            prompt_snippet: Some(
                "Search for tools that are not loaded yet and load the matches".to_string(),
            ),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query for deferred tools."},
                    "limit": {"type": "number", "description": "Maximum number of tools to return. Defaults to 8."}
                },
                "required": ["query"]
            })),
            exposure: ToolExposure::Direct,
            ..Default::default()
        };
        let tool = Arc::new(pi_agent_core::types::AgentTool {
            constrained_sampling: None,
            name: definition.name.clone(),
            description: definition.description.clone(),
            label: definition.label.clone().unwrap_or_default(),
            prompt_snippet: definition.prompt_snippet.clone(),
            prompt_guidelines: None,
            parameters_schema: definition
                .parameters
                .clone()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}, "required": []})),
            execution_mode: None,
            prepare_arguments: None,
            execute,
        }) as Arc<DynTool>;
        {
            let mut registry = self
                .registry
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !registry.iter().any(|tool| tool.name == "tool_search") {
                registry.push(Arc::clone(&tool));
            }
            let mut definitions = self
                .definitions
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            definitions.insert("tool_search".to_string(), definition);
        }
        let agent = self
            .agent
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(agent) = agent {
            agent.add_tools(vec![tool]).await;
        }
    }

    /// Add initial searchable tools to the index.
    pub fn seed_search(&self, documents: Vec<ToolSearchDocument>, registry: HashMap<String, Arc<DynTool>>) {
        let mut search = self
            .search
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        search.documents.extend(documents);
        search.registry.extend(registry);
        self.registered.store(true, Ordering::SeqCst);
    }

    /// Register or replace tools.
    pub async fn register(self: &Arc<Self>, definitions: Vec<ToolDefinition>) {
        if definitions.is_empty() {
            return;
        }
        if definitions.iter().any(|definition| searchable(definition.exposure)) {
            self.ensure_tool_search().await;
        }
        let mut direct_tools = Vec::new();
        {
            let mut registry = self
                .registry
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut definitions_map = self
                .definitions
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut search = self
                .search
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for definition in definitions {
                let tool = Arc::new(agent_tool_from_definition(&definition)) as Arc<DynTool>;
                match registry.iter_mut().find(|tool| tool.name == definition.name) {
                    Some(existing) => *existing = Arc::clone(&tool),
                    None => registry.push(Arc::clone(&tool)),
                }
                // Refresh the search index entry.
                search.documents.retain(|doc| doc.name != definition.name);
                search.registry.remove(&definition.name);
                if searchable(definition.exposure) {
                    let namespace = definition.namespace.as_ref().map(|namespace| {
                        (
                            namespace.name.as_str(),
                            namespace.description.as_deref(),
                            namespace.instructions.as_deref(),
                        )
                    });
                    search.documents.push(create_tool_search_document(
                        &definition.name,
                        &definition.description,
                        definition.parameters.as_ref(),
                        namespace,
                    ));
                    search.registry.insert(definition.name.clone(), Arc::clone(&tool));
                }
                if direct(definition.exposure) {
                    direct_tools.push(Arc::clone(&tool));
                }
                definitions_map.insert(definition.name.clone(), definition);
            }
            self.registered.store(true, Ordering::SeqCst);
        }
        if !direct_tools.is_empty() {
            let agent = self
                .agent
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if let Some(agent) = agent {
                agent.add_tools(direct_tools).await;
            }
        }
    }

    /// Remove tools by name.
    pub async fn unregister(&self, names: &[String]) {
        if names.is_empty() {
            return;
        }
        let names_set: HashSet<&str> = names.iter().map(String::as_str).collect();
        {
            let mut registry = self
                .registry
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            registry.retain(|tool| !names_set.contains(tool.name.as_str()));
            let mut definitions = self
                .definitions
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            definitions.retain(|name, _| !names_set.contains(name.as_str()));
            let mut search = self
                .search
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            search
                .documents
                .retain(|document| !names_set.contains(document.name.as_str()));
            search
                .registry
                .retain(|name, _| !names_set.contains(name.as_str()));
        }
        let agent = self
            .agent
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(agent) = agent {
            let names: HashSet<String> = names.iter().cloned().collect();
            agent
                .update_state(|state| state.tools.retain(|tool| !names.contains(&tool.name)))
                .await;
        }
    }

    /// All runtime definitions.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.definitions
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// One runtime definition.
    pub fn definition(&self, name: &str) -> Option<ToolDefinition> {
        self.definitions
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(name)
            .cloned()
    }

    /// One runtime tool.
    pub fn tool(&self, name: &str) -> Option<Arc<DynTool>> {
        self.registry
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|tool| tool.name == name)
            .cloned()
    }

    /// All runtime tool Arcs.
    pub fn tools(&self) -> Vec<Arc<DynTool>> {
        self.registry
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Rank searchable runtime tools that are not already active.
    pub fn search(
        &self,
        query: &str,
        active: &HashSet<String>,
        limit: usize,
    ) -> Vec<(String, Arc<DynTool>)> {
        let search = self
            .search
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let candidates: Vec<ToolSearchDocument> = search
            .documents
            .iter()
            .filter(|document| !active.contains(&document.name))
            .cloned()
            .collect();
        Bm25Ranker::default()
            .rank(query, &candidates, limit)
            .into_iter()
            .filter_map(|matched| {
                search
                    .registry
                    .get(&matched.name)
                    .map(|tool| (matched.name, Arc::clone(tool)))
            })
            .collect()
    }

    /// Whether any tools have been registered at runtime.
    pub fn has_registered(&self) -> bool {
        self.registered.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn definition(name: &str, exposure: ToolExposure) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("{name} description"),
            parameters: Some(json!({"type": "object", "properties": {}})),
            exposure,
            ..Default::default()
        }
    }

    #[test]
    fn searchable_tools_enter_the_index() {
        let manager = std::sync::Arc::new(RuntimeToolManager::default());
        manager.seed_search(Vec::new(), HashMap::new());
        futures::executor::block_on(manager.register(vec![definition(
            "search_me",
            ToolExposure::Deferred,
        )]));
        let matches = manager.search("search", &HashSet::new(), 8);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].0, "search_me");
        assert!(manager.definition("search_me").is_some());
    }

    #[tokio::test]
    async fn unregister_removes_definition() {
        let manager = std::sync::Arc::new(RuntimeToolManager::default());
        manager
            .register(vec![definition("gone", ToolExposure::Deferred)])
            .await;
        manager.unregister(&["gone".to_string()]).await;
        assert!(manager.definition("gone").is_none());
        assert!(manager.search("gone", &HashSet::new(), 8).is_empty());
    }
}
