pub mod agent_harness;
pub mod compaction;
pub mod env;
pub mod messages;
pub mod prompt_templates;
pub mod session;
pub mod skill_loader;
pub mod skills;
pub mod system_prompt;
pub mod types;
pub mod utils;

pub use agent_harness::{AgentHarness, AgentHarnessEvent, AgentHarnessOptions};
pub use compaction::branch_summarization::{
    collect_entries_for_branch_summary, generate_branch_summary, prepare_branch_entries,
    BranchPreparation,
};
pub use compaction::compaction::{
    calculate_context_tokens, compact, estimate_context_tokens, estimate_tokens, find_cut_point,
    find_turn_start_index, generate_summary, get_last_assistant_usage, prepare_compaction,
    serialize_conversation, should_compact,
};
pub use compaction::utils::{
    compute_file_lists, create_file_ops, extract_file_ops_from_message, format_file_operations,
};
pub use env::nodejs::NodeExecutionEnv;
pub use messages::{
    bash_execution_to_text, convert_to_llm, create_branch_summary_message,
    create_compaction_summary_message, create_custom_message, BRANCH_SUMMARY_PREFIX,
    BRANCH_SUMMARY_SUFFIX, COMPACTION_SUMMARY_PREFIX, COMPACTION_SUMMARY_SUFFIX,
};
pub use prompt_templates::{
    format_prompt_template_invocation, parse_command_args, substitute_args,
};
pub use session::jsonl_repo::JsonlSessionRepo;
pub use session::jsonl_storage::JsonlSessionStorage;
pub use session::memory_repo::InMemorySessionRepo;
pub use session::memory_storage::InMemorySessionStorage;
pub use session::repo_utils::{create_session_id, create_timestamp, to_session};
pub use skills::format_skills_for_system_prompt;
pub use system_prompt::format_skills_for_system_prompt as format_skills_for_system_prompt_v2;
pub use types::*;
