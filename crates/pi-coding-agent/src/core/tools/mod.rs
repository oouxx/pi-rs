pub mod bash;
pub mod edit;
pub mod edit_diff;
pub mod file_mutation_queue;
pub mod find;
pub mod grep;
pub mod ls;
pub mod output_accumulator;
pub mod path_utils;
pub mod read;
pub mod render_utils;
pub mod tool_definition_wrapper;
pub mod truncate;
pub mod write;

use pi_agent_core::types::AgentTool;
use serde::{Deserialize, Serialize};

pub use output_accumulator::{OutputAccumulator, OutputAccumulatorOptions, OutputSnapshot};
pub use truncate::{TruncationOptions, TruncationResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolName {
    Read,
    Bash,
    Edit,
    Write,
    Grep,
    Find,
    Ls,
}

impl ToolName {
    pub fn all() -> Vec<ToolName> {
        vec![
            ToolName::Read,
            ToolName::Bash,
            ToolName::Edit,
            ToolName::Write,
            ToolName::Grep,
            ToolName::Find,
            ToolName::Ls,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ToolName::Read => "read",
            ToolName::Bash => "bash",
            ToolName::Edit => "edit",
            ToolName::Write => "write",
            ToolName::Grep => "grep",
            ToolName::Find => "find",
            ToolName::Ls => "ls",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ToolsOptions {
    pub read: Option<read::ReadToolOptions>,
    pub bash: Option<bash::BashToolOptions>,
    pub write: Option<write::WriteToolOptions>,
    pub edit: Option<edit::EditToolOptions>,
    pub grep: Option<grep::GrepToolOptions>,
    pub find: Option<find::FindToolOptions>,
    pub ls: Option<ls::LsToolOptions>,
}

pub type DynTool = AgentTool<serde_json::Value, serde_json::Value>;

pub fn create_coding_tools(cwd: &str, options: Option<&ToolsOptions>) -> Vec<DynTool> {
    let opts = options.cloned().unwrap_or_default();
    vec![
        read::create_read_tool(cwd, opts.read),
        bash::create_bash_tool(cwd, opts.bash),
        edit::create_edit_tool(cwd, opts.edit),
        write::create_write_tool(cwd, opts.write),
    ]
}

pub fn create_read_only_tools(cwd: &str, options: Option<&ToolsOptions>) -> Vec<DynTool> {
    let opts = options.cloned().unwrap_or_default();
    vec![
        read::create_read_tool(cwd, opts.read),
        grep::create_grep_tool(cwd, opts.grep),
        find::create_find_tool(cwd, opts.find),
        ls::create_ls_tool(cwd, opts.ls),
    ]
}