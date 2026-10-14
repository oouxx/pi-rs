/// Constrained sampling the built-in `read`, `bash`, `edit` and `write` tools
/// declare: strict-prefer JSON-schema sampling, enabled by default without
/// `PI_EXPERIMENTAL` (TS enables it on those four tool definitions). `grep`,
/// `find` and `ls` deliberately declare none, matching TS.
#[must_use]
pub fn preferred_json_schema_sampling() -> Option<pi_ai::types::ConstrainedSamplingConfig> {
    Some(pi_ai::types::ConstrainedSamplingConfig::JsonSchema {
        strict: pi_ai::types::JsonSchemaStrict::Prefer,
    })
}

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

/// Async operation result used by the pluggable tool operation traits
/// (read/ls/grep/find/bash/edit/write).
pub type AsyncOpResult<T> = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<T, Box<dyn std::error::Error + Send + Sync>>,
            > + Send,
    >,
>;

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

pub fn create_coding_tools(
    cwd: &str,
    options: Option<&ToolsOptions>,
    bash_session_env_provider: Option<bash::BashSessionEnvProvider>,
) -> Vec<DynTool> {
    let opts = options.cloned().unwrap_or_default();
    let mut bash_opts = opts.bash.unwrap_or_default();
    if bash_session_env_provider.is_some() {
        bash_opts.session_env_provider = bash_session_env_provider;
    }
    vec![
        read::create_read_tool(cwd, opts.read),
        bash::create_bash_tool(cwd, Some(bash_opts)),
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

#[cfg(test)]
mod sampling_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use pi_ai::types::{ConstrainedSamplingConfig, JsonSchemaStrict};

    /// The built-in tools declare strict-prefer JSON-schema sampling by default
    /// — only the four the original declares it on. `grep`, `find` and `ls`
    /// must stay unconstrained (TS `read`/`bash`/`edit`/`write` only).
    #[test]
    fn builtin_tool_sampling_matches_ts() {
        let expected = Some(ConstrainedSamplingConfig::JsonSchema {
            strict: JsonSchemaStrict::Prefer,
        });
        let strict = [
            super::read::create_read_tool("/tmp", None).constrained_sampling,
            super::bash::create_bash_tool("/tmp", None).constrained_sampling,
            super::edit::create_edit_tool("/tmp", None).constrained_sampling,
            super::write::create_write_tool("/tmp", None).constrained_sampling,
        ];
        for (index, sampling) in strict.iter().enumerate() {
            assert_eq!(
                sampling, &expected,
                "built-in tool {index} must declare strict-prefer sampling"
            );
        }

        let unconstrained = [
            super::grep::create_grep_tool("/tmp", None).constrained_sampling,
            super::find::create_find_tool("/tmp", None).constrained_sampling,
            super::ls::create_ls_tool("/tmp", None).constrained_sampling,
        ];
        for (index, sampling) in unconstrained.iter().enumerate() {
            assert!(sampling.is_none(), "grep/find/ls entry {index} must stay unconstrained");
        }
    }
}
