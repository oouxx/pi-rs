pub mod api;
pub mod dispatcher;
pub mod types;

pub use api::{
    ArgumentCompletionsFn, AutocompleteItem, CommandRegistry, CommandRegistration, EventPublisher,
    ExtensionContext, ExtensionRegistry, ExtensionUIContext, FlagRegistry, HookHandler, HookResult,
    HookRunner, RegisteredCommand, RegisteredFlag, RegisteredShortcut, RegisteredTool, RuntimeHandle,
    SendMessageOptions, SendUserMessageOptions, ShortcutRegistry, ToolCallOutput, ToolDefinition,
    ToolInfo, ToolRegistry,
};
pub use api::{create_builtin_source_info, create_source_info, create_synthetic_source_info, SourceInfo, SourceOrigin, SourceScope};
pub use api::ResourcesDiscoverResult;
pub use api::{ProjectTrustDecision, ProjectTrustResult, UserBashResult};
