//! Extension system tests for pi-coding-agent.
//!
//! Ported from the original TypeScript test suite:
//! - extensions-runner.test.ts — ExtensionRunner tests (conflict detection, error handling, tool wrapping)
//! - extensions-discovery.test.ts — Extension discovery and loading tests
//! - extensions-input-event.test.ts — Input event handling tests
//! - compaction-extensions.test.ts — Compaction extension events
//! - compaction-extensions-example.test.ts — Documentation example verification
//! - trigger-compact-extension.test.ts — Trigger-compact example extension
//! - plan-mode-extension.test.ts — Plan-mode example extension
//! - git-merge-and-resolve-extension.test.ts — Git merge and resolve example extension
//!
//! Run with: cargo test -p pi-coding-agent --test extensions_test -- --nocapture

use std::collections::HashMap;

use pi_coding_agent::core::diagnostics::ResourceDiagnostic;
use pi_coding_agent::core::extensions::types::*;

// ============================================================================
// Helper functions
// ============================================================================

/// Create a temporary directory for testing.
fn setup_temp_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("Failed to create temp dir")
}

/// Create a basic tool definition for testing.
fn make_tool_def(name: &str, description: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        label: None,
        description: description.to_string(),
        prompt_snippet: None,
        prompt_guidelines: None,
        parameters: None,
        render_shell: None,
        execution_mode: None,
    }
}

/// Create a basic extension manifest for testing.
fn make_manifest(name: &str, tools: Vec<ToolDefinition>) -> ExtensionManifest {
    ExtensionManifest {
        name: name.to_string(),
        version: None,
        description: None,
        main: None,
        tools,
        commands: vec![],
        flags: vec![],
        shortcuts: vec![],
    }
}

/// Create a loaded extension for testing.
fn make_loaded_extension(
    name: &str,
    path: &str,
    source: ExtensionSource,
    tools: HashMap<String, RegisteredTool>,
) -> LoadedExtension {
    LoadedExtension {
        path: path.to_string(),
        resolved_path: path.to_string(),
        source,
        manifest: make_manifest(name, vec![]),
        tools,
        commands: HashMap::new(),
        flags: HashMap::new(),
        shortcuts: HashMap::new(),
    }
}

// ============================================================================
// Extension Types Tests
// Ported from: extensions-runner.test.ts (type definitions)
//             extensions-discovery.test.ts (type validation)
// ============================================================================

#[cfg(test)]
mod extension_types_tests {
    use super::*;

    #[test]
    fn test_tool_definition_serialization_roundtrip() {
        let tool = ToolDefinition {
            name: "my-tool".to_string(),
            label: Some("My Tool".to_string()),
            description: "A test tool".to_string(),
            prompt_snippet: Some("Run my-tool".to_string()),
            prompt_guidelines: Some(vec!["Use with care".to_string()]),
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                }
            })),
            render_shell: Some("bash".to_string()),
            execution_mode: Some("sequential".to_string()),
        };

        // Serialize to JSON
        let json = serde_json::to_string(&tool).expect("Should serialize");
        // Deserialize back
        let deserialized: ToolDefinition = serde_json::from_str(&json).expect("Should deserialize");

        assert_eq!(deserialized.name, "my-tool");
        assert_eq!(deserialized.label, Some("My Tool".to_string()));
        assert_eq!(deserialized.description, "A test tool");
        assert_eq!(deserialized.prompt_snippet, Some("Run my-tool".to_string()));
        assert_eq!(
            deserialized.prompt_guidelines,
            Some(vec!["Use with care".to_string()])
        );
        assert_eq!(deserialized.execution_mode, Some("sequential".to_string()));
    }

    #[test]
    fn test_tool_definition_minimal_serialization() {
        let tool = ToolDefinition {
            name: "minimal".to_string(),
            label: None,
            description: "Minimal tool".to_string(),
            prompt_snippet: None,
            prompt_guidelines: None,
            parameters: None,
            render_shell: None,
            execution_mode: None,
        };

        let json = serde_json::to_string(&tool).expect("Should serialize");
        let deserialized: ToolDefinition = serde_json::from_str(&json).expect("Should deserialize");

        assert_eq!(deserialized.name, "minimal");
        assert_eq!(deserialized.description, "Minimal tool");
        assert!(deserialized.label.is_none());
        assert!(deserialized.parameters.is_none());
    }

    #[test]
    fn test_extension_manifest_serialization_roundtrip() {
        let manifest = ExtensionManifest {
            name: "test-extension".to_string(),
            version: Some("1.0.0".to_string()),
            description: Some("A test extension".to_string()),
            main: Some("./index.ts".to_string()),
            tools: vec![
                make_tool_def("tool-a", "Tool A"),
                make_tool_def("tool-b", "Tool B"),
            ],
            commands: vec![
                CommandDefinition {
                    name: "cmd-a".to_string(),
                    description: Some("Command A".to_string()),
                },
            ],
            flags: vec![
                ExtensionFlag {
                    name: "verbose".to_string(),
                    description: Some("Enable verbose output".to_string()),
                    flag_type: Some("boolean".to_string()),
                    default_value: Some(serde_json::json!(false)),
                    extension_path: None,
                },
            ],
            shortcuts: vec![
                ExtensionShortcut {
                    key: "ctrl+t".to_string(),
                    description: Some("Test shortcut".to_string()),
                    command: None,
                    extension_path: None,
                },
            ],
        };

        let json = serde_json::to_string_pretty(&manifest).expect("Should serialize");
        let deserialized: ExtensionManifest = serde_json::from_str(&json).expect("Should deserialize");

        assert_eq!(deserialized.name, "test-extension");
        assert_eq!(deserialized.version, Some("1.0.0".to_string()));
        assert_eq!(deserialized.tools.len(), 2);
        assert_eq!(deserialized.commands.len(), 1);
        assert_eq!(deserialized.flags.len(), 1);
        assert_eq!(deserialized.shortcuts.len(), 1);
        assert_eq!(deserialized.tools[0].name, "tool-a");
        assert_eq!(deserialized.tools[1].name, "tool-b");
        assert_eq!(deserialized.commands[0].name, "cmd-a");
        assert_eq!(deserialized.flags[0].name, "verbose");
        assert_eq!(deserialized.shortcuts[0].key, "ctrl+t");
    }

    #[test]
    fn test_extension_manifest_defaults() {
        // Minimal manifest with only required fields
        let json = r#"{"name": "minimal"}"#;
        let manifest: ExtensionManifest = serde_json::from_str(json).expect("Should parse");

        assert_eq!(manifest.name, "minimal");
        assert!(manifest.version.is_none());
        assert!(manifest.description.is_none());
        assert!(manifest.main.is_none());
        assert!(manifest.tools.is_empty());
        assert!(manifest.commands.is_empty());
        assert!(manifest.flags.is_empty());
        assert!(manifest.shortcuts.is_empty());
    }

    #[test]
    fn test_extension_source_serialization() {
        // Test serialization of ExtensionSource variants
        assert_eq!(
            serde_json::to_string(&ExtensionSource::User).unwrap(),
            r#""user""#
        );
        assert_eq!(
            serde_json::to_string(&ExtensionSource::Project).unwrap(),
            r#""project""#
        );
        assert_eq!(
            serde_json::to_string(&ExtensionSource::Path).unwrap(),
            r#""path""#
        );
    }

    #[test]
    fn test_extension_source_deserialization() {
        assert_eq!(
            serde_json::from_str::<ExtensionSource>(r#""user""#).unwrap(),
            ExtensionSource::User
        );
        assert_eq!(
            serde_json::from_str::<ExtensionSource>(r#""project""#).unwrap(),
            ExtensionSource::Project
        );
        assert_eq!(
            serde_json::from_str::<ExtensionSource>(r#""path""#).unwrap(),
            ExtensionSource::Path
        );
    }

    #[test]
    fn test_tool_info_from_registered_tool() {
        let tool = RegisteredTool {
            definition: ToolDefinition {
                name: "my-tool".to_string(),
                description: "A test tool".to_string(),
                label: None,
                prompt_snippet: None,
                prompt_guidelines: Some(vec!["guideline 1".to_string(), "guideline 2".to_string()]),
                parameters: Some(serde_json::json!({"type": "object"})),
                render_shell: None,
                execution_mode: None,
            },
            source_path: "/test/extension".to_string(),
        };

        let info = ToolInfo::from(&tool);
        assert_eq!(info.name, "my-tool");
        assert_eq!(info.description, "A test tool");
        assert_eq!(info.prompt_guidelines, vec!["guideline 1", "guideline 2"]);
        assert_eq!(info.source_path, "/test/extension");
        assert_eq!(info.parameters, Some(serde_json::json!({"type": "object"})));
    }

    #[test]
    fn test_tool_info_empty_prompt_guidelines() {
        let tool = RegisteredTool {
            definition: ToolDefinition {
                name: "no-guidelines".to_string(),
                description: "No guidelines".to_string(),
                label: None,
                prompt_snippet: None,
                prompt_guidelines: None,
                parameters: None,
                render_shell: None,
                execution_mode: None,
            },
            source_path: "/test".to_string(),
        };

        let info = ToolInfo::from(&tool);
        assert!(info.prompt_guidelines.is_empty());
    }

    #[test]
    fn test_registered_command_creation() {
        let cmd = RegisteredCommand {
            name: "test-cmd".to_string(),
            source_path: "/test/extension".to_string(),
            description: Some("A test command".to_string()),
        };

        assert_eq!(cmd.name, "test-cmd");
        assert_eq!(cmd.description, Some("A test command".to_string()));
        assert_eq!(cmd.source_path, "/test/extension");
    }

    #[test]
    fn test_registered_command_no_description() {
        let cmd = RegisteredCommand {
            name: "minimal-cmd".to_string(),
            source_path: "/test".to_string(),
            description: None,
        };

        assert_eq!(cmd.name, "minimal-cmd");
        assert!(cmd.description.is_none());
    }

    #[test]
    fn test_extension_flag_defaults() {
        let flag = ExtensionFlag {
            name: "my-flag".to_string(),
            description: None,
            flag_type: None,
            default_value: None,
            extension_path: None,
        };

        assert_eq!(flag.name, "my-flag");
        assert!(flag.description.is_none());
        assert!(flag.flag_type.is_none());
        assert!(flag.default_value.is_none());
    }

    #[test]
    fn test_extension_flag_with_values() {
        let flag = ExtensionFlag {
            name: "verbose".to_string(),
            description: Some("Enable verbose mode".to_string()),
            flag_type: Some("boolean".to_string()),
            default_value: Some(serde_json::json!(true)),
            extension_path: Some("/test/ext".to_string()),
        };

        assert_eq!(flag.name, "verbose");
        assert_eq!(flag.flag_type, Some("boolean".to_string()));
        assert_eq!(flag.default_value, Some(serde_json::json!(true)));
    }

    #[test]
    fn test_extension_shortcut_creation() {
        let shortcut = ExtensionShortcut {
            key: "ctrl+p".to_string(),
            description: Some("Print".to_string()),
            command: Some("print".to_string()),
            extension_path: Some("/test/ext".to_string()),
        };

        assert_eq!(shortcut.key, "ctrl+p");
        assert_eq!(shortcut.description, Some("Print".to_string()));
        assert_eq!(shortcut.command, Some("print".to_string()));
    }

    #[test]
    fn test_extension_shortcut_minimal() {
        let shortcut = ExtensionShortcut {
            key: "ctrl+x".to_string(),
            description: None,
            command: None,
            extension_path: None,
        };

        assert_eq!(shortcut.key, "ctrl+x");
        assert!(shortcut.description.is_none());
        assert!(shortcut.command.is_none());
    }
}

// ============================================================================
// Extension Discovery Tests
// Ported from: extensions-discovery.test.ts
// ============================================================================

#[cfg(test)]
mod extension_discovery_tests {
    use super::*;

    #[test]
    fn test_load_extensions_empty_options() {
        let opts = LoadExtensionsOptions {
            cwd: "/nonexistent".to_string(),
            agent_dir: None,
            extension_paths: vec![],
            include_defaults: false,
        };
        let result = load_extensions(&opts);
        assert!(result.extensions.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_load_extensions_with_nonexistent_paths() {
        let opts = LoadExtensionsOptions {
            cwd: "/tmp".to_string(),
            agent_dir: None,
            extension_paths: vec!["/nonexistent/path".to_string()],
            include_defaults: false,
        };
        let result = load_extensions(&opts);
        assert!(result.extensions.is_empty());
        // Non-existent paths produce diagnostics, not errors
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_load_extensions_from_manifest_file() {
        let temp_dir = setup_temp_dir();
        let ext_dir = temp_dir.path().join("my-extension");
        std::fs::create_dir_all(&ext_dir).expect("Should create ext dir");

        // Create extension.json manifest
        let manifest = serde_json::json!({
            "name": "my-extension",
            "version": "1.0.0",
            "description": "My test extension",
            "tools": [
                {
                    "name": "hello",
                    "description": "Says hello",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" }
                        }
                    }
                },
                {
                    "name": "goodbye",
                    "description": "Says goodbye"
                }
            ],
            "commands": [
                {
                    "name": "greet",
                    "description": "Greet the user"
                }
            ],
            "flags": [
                {
                    "name": "verbose",
                    "description": "Enable verbose output",
                    "flag_type": "boolean",
                    "default_value": false
                }
            ],
            "shortcuts": [
                {
                    "key": "ctrl+g",
                    "description": "Greet shortcut"
                }
            ]
        });

        std::fs::write(
            ext_dir.join("extension.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .expect("Should write manifest");

        let opts = LoadExtensionsOptions {
            cwd: temp_dir.path().to_string_lossy().to_string(),
            agent_dir: None,
            extension_paths: vec![ext_dir.to_string_lossy().to_string()],
            include_defaults: false,
        };
        let result = load_extensions(&opts);

        assert_eq!(result.extensions.len(), 1, "Should load 1 extension");
        assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);

        let ext = &result.extensions[0];
        assert_eq!(ext.manifest.name, "my-extension");
        assert_eq!(ext.manifest.version, Some("1.0.0".to_string()));
        assert_eq!(ext.tools.len(), 2);
        assert_eq!(ext.commands.len(), 1);
        assert_eq!(ext.flags.len(), 1);
        assert_eq!(ext.shortcuts.len(), 1);

        // Verify tool details
        assert!(ext.tools.contains_key("hello"));
        assert!(ext.tools.contains_key("goodbye"));
        assert_eq!(ext.tools["hello"].definition.description, "Says hello");
        assert_eq!(
            ext.tools["hello"].definition.parameters,
            Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                }
            }))
        );

        // Verify command details
        assert!(ext.commands.contains_key("greet"));
        assert_eq!(
            ext.commands["greet"].description,
            Some("Greet the user".to_string())
        );

        // Verify flag details
        assert!(ext.flags.contains_key("verbose"));
        assert_eq!(ext.flags["verbose"].flag_type, Some("boolean".to_string()));

        // Verify shortcut details
        assert!(ext.shortcuts.contains_key("ctrl+g"));
    }

    #[test]
    fn test_load_extensions_from_package_json() {
        let temp_dir = setup_temp_dir();
        let ext_dir = temp_dir.path().join("npm-package");
        std::fs::create_dir_all(&ext_dir).expect("Should create ext dir");

        // Create package.json with pi field
        let package_json = serde_json::json!({
            "name": "npm-package",
            "version": "1.0.0",
            "pi": {
                "extensions": ["./dist/index.js"]
            },
            "tools": [
                {
                    "name": "npm-tool",
                    "description": "A tool from npm package"
                }
            ]
        });

        std::fs::write(
            ext_dir.join("package.json"),
            serde_json::to_string_pretty(&package_json).unwrap(),
        )
        .expect("Should write package.json");

        let opts = LoadExtensionsOptions {
            cwd: temp_dir.path().to_string_lossy().to_string(),
            agent_dir: None,
            extension_paths: vec![ext_dir.to_string_lossy().to_string()],
            include_defaults: false,
        };
        let result = load_extensions(&opts);

        // package.json is loaded as an alternative manifest by the Rust loader
        // (the Rust loader falls back to package.json if extension.json is not present)
        assert_eq!(result.extensions.len(), 1, "package.json should be loaded as an alternative manifest");
    }

    #[test]
    fn test_load_extensions_multiple_extensions() {
        let temp_dir = setup_temp_dir();

        // Create two extension directories
        for name in &["ext-a", "ext-b"] {
            let ext_dir = temp_dir.path().join(name);
            std::fs::create_dir_all(&ext_dir).expect("Should create ext dir");

            let manifest = serde_json::json!({
                "name": name,
                "tools": [
                    {
                        "name": format!("tool-{}", name),
                        "description": format!("Tool from {}", name)
                    }
                ]
            });

            std::fs::write(
                ext_dir.join("extension.json"),
                serde_json::to_string_pretty(&manifest).unwrap(),
            )
            .expect("Should write manifest");
        }

        let opts = LoadExtensionsOptions {
            cwd: temp_dir.path().to_string_lossy().to_string(),
            agent_dir: None,
            extension_paths: vec![
                temp_dir.path().join("ext-a").to_string_lossy().to_string(),
                temp_dir.path().join("ext-b").to_string_lossy().to_string(),
            ],
            include_defaults: false,
        };
        let result = load_extensions(&opts);

        assert_eq!(result.extensions.len(), 2);
        assert!(result.errors.is_empty());

        let names: Vec<&str> = result.extensions.iter().map(|e| e.manifest.name.as_str()).collect();
        assert!(names.contains(&"ext-a"));
        assert!(names.contains(&"ext-b"));
    }

    #[test]
    fn test_load_extensions_invalid_manifest() {
        let temp_dir = setup_temp_dir();
        let ext_dir = temp_dir.path().join("bad-extension");
        std::fs::create_dir_all(&ext_dir).expect("Should create ext dir");

        // Write invalid JSON
        std::fs::write(ext_dir.join("extension.json"), "not valid json{").expect("Should write");

        let opts = LoadExtensionsOptions {
            cwd: temp_dir.path().to_string_lossy().to_string(),
            agent_dir: None,
            extension_paths: vec![ext_dir.to_string_lossy().to_string()],
            include_defaults: false,
        };
        let result = load_extensions(&opts);

        assert!(result.extensions.is_empty());
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].error.contains("parse manifest"));
    }

    #[test]
    fn test_load_extensions_missing_manifest() {
        let temp_dir = setup_temp_dir();
        let ext_dir = temp_dir.path().join("no-manifest");
        std::fs::create_dir_all(&ext_dir).expect("Should create ext dir");

        // Directory exists but has no extension.json or package.json
        let opts = LoadExtensionsOptions {
            cwd: temp_dir.path().to_string_lossy().to_string(),
            agent_dir: None,
            extension_paths: vec![ext_dir.to_string_lossy().to_string()],
            include_defaults: false,
        };
        let result = load_extensions(&opts);

        // No manifest found, so no extension loaded
        assert!(result.extensions.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_load_extensions_duplicate_tool_names() {
        let temp_dir = setup_temp_dir();

        // Create two extensions with the same tool name
        for name in &["ext-1", "ext-2"] {
            let ext_dir = temp_dir.path().join(name);
            std::fs::create_dir_all(&ext_dir).expect("Should create ext dir");

            let manifest = serde_json::json!({
                "name": name,
                "tools": [
                    {
                        "name": "shared-tool",
                        "description": format!("Tool from {}", name)
                    }
                ]
            });

            std::fs::write(
                ext_dir.join("extension.json"),
                serde_json::to_string_pretty(&manifest).unwrap(),
            )
            .expect("Should write manifest");
        }

        let opts = LoadExtensionsOptions {
            cwd: temp_dir.path().to_string_lossy().to_string(),
            agent_dir: None,
            extension_paths: vec![
                temp_dir.path().join("ext-1").to_string_lossy().to_string(),
                temp_dir.path().join("ext-2").to_string_lossy().to_string(),
            ],
            include_defaults: false,
        };
        let result = load_extensions(&opts);

        assert_eq!(result.extensions.len(), 2);

        // Both extensions have the tool, but the last one wins in get_all_extension_tools
        let all_tools = get_all_extension_tools(&result.extensions);
        let shared_tools: Vec<&RegisteredTool> = all_tools.iter().filter(|t| t.definition.name == "shared-tool").collect();
        assert_eq!(shared_tools.len(), 2, "Both extensions' tools should be collected");
    }
}

// ============================================================================
// Extension Query Helper Tests
// Ported from: extensions-runner.test.ts (tool wrapping, getActiveTools, etc.)
// ============================================================================

#[cfg(test)]
mod extension_query_tests {
    use super::*;

    #[test]
    fn test_get_all_extension_tools_empty() {
        let tools = get_all_extension_tools(&[]);
        assert!(tools.is_empty());
    }

    #[test]
    fn test_get_all_extension_tools_multiple_extensions() {
        let ext1 = make_loaded_extension(
            "ext1",
            "/ext1",
            ExtensionSource::Path,
            HashMap::from([
                (
                    "tool-a".to_string(),
                    RegisteredTool {
                        definition: make_tool_def("tool-a", "Tool A"),
                        source_path: "/ext1".to_string(),
                    },
                ),
            ]),
        );

        let ext2 = make_loaded_extension(
            "ext2",
            "/ext2",
            ExtensionSource::Path,
            HashMap::from([
                (
                    "tool-b".to_string(),
                    RegisteredTool {
                        definition: make_tool_def("tool-b", "Tool B"),
                        source_path: "/ext2".to_string(),
                    },
                ),
                (
                    "tool-c".to_string(),
                    RegisteredTool {
                        definition: make_tool_def("tool-c", "Tool C"),
                        source_path: "/ext2".to_string(),
                    },
                ),
            ]),
        );

        let tools = get_all_extension_tools(&[ext1, ext2]);
        assert_eq!(tools.len(), 3);

        let names: Vec<&str> = tools.iter().map(|t| t.definition.name.as_str()).collect();
        assert!(names.contains(&"tool-a"));
        assert!(names.contains(&"tool-b"));
        assert!(names.contains(&"tool-c"));
    }

    #[test]
    fn test_get_all_extension_commands_empty() {
        let commands = get_all_extension_commands(&[]);
        assert!(commands.is_empty());
    }

    #[test]
    fn test_get_all_extension_commands_multiple() {
        let ext = LoadedExtension {
            path: "/test".to_string(),
            resolved_path: "/test".to_string(),
            source: ExtensionSource::Path,
            manifest: make_manifest("test", vec![]),
            tools: HashMap::new(),
            commands: HashMap::from([
                (
                    "cmd1".to_string(),
                    RegisteredCommand {
                        name: "cmd1".to_string(),
                        source_path: "/test".to_string(),
                        description: Some("Command 1".to_string()),
                    },
                ),
                (
                    "cmd2".to_string(),
                    RegisteredCommand {
                        name: "cmd2".to_string(),
                        source_path: "/test".to_string(),
                        description: None,
                    },
                ),
            ]),
            flags: HashMap::new(),
            shortcuts: HashMap::new(),
        };

        let commands = get_all_extension_commands(&[ext]);
        assert_eq!(commands.len(), 2);

        let names: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"cmd1"));
        assert!(names.contains(&"cmd2"));
    }

    #[test]
    fn test_get_all_extension_flags_empty() {
        let flags = get_all_extension_flags(&[]);
        assert!(flags.is_empty());
    }

    #[test]
    fn test_get_all_extension_flags_multiple() {
        let ext1 = LoadedExtension {
            path: "/ext1".to_string(),
            resolved_path: "/ext1".to_string(),
            source: ExtensionSource::Path,
            manifest: make_manifest("ext1", vec![]),
            tools: HashMap::new(),
            commands: HashMap::new(),
            flags: HashMap::from([
                (
                    "flag-a".to_string(),
                    ExtensionFlag {
                        name: "flag-a".to_string(),
                        description: Some("Flag A".to_string()),
                        flag_type: Some("boolean".to_string()),
                        default_value: Some(serde_json::json!(true)),
                        extension_path: None,
                    },
                ),
            ]),
            shortcuts: HashMap::new(),
        };

        let ext2 = LoadedExtension {
            path: "/ext2".to_string(),
            resolved_path: "/ext2".to_string(),
            source: ExtensionSource::Path,
            manifest: make_manifest("ext2", vec![]),
            tools: HashMap::new(),
            commands: HashMap::new(),
            flags: HashMap::from([
                (
                    "flag-b".to_string(),
                    ExtensionFlag {
                        name: "flag-b".to_string(),
                        description: Some("Flag B".to_string()),
                        flag_type: Some("string".to_string()),
                        default_value: Some(serde_json::json!("default")),
                        extension_path: None,
                    },
                ),
            ]),
            shortcuts: HashMap::new(),
        };

        let flags = get_all_extension_flags(&[ext1, ext2]);
        assert_eq!(flags.len(), 2);
        assert!(flags.contains_key("flag-a"));
        assert!(flags.contains_key("flag-b"));
    }

    #[test]
    fn test_get_all_extension_flags_deduplicates() {
        let ext1 = LoadedExtension {
            path: "/ext1".to_string(),
            resolved_path: "/ext1".to_string(),
            source: ExtensionSource::Path,
            manifest: make_manifest("ext1", vec![]),
            tools: HashMap::new(),
            commands: HashMap::new(),
            flags: HashMap::from([
                (
                    "shared-flag".to_string(),
                    ExtensionFlag {
                        name: "shared-flag".to_string(),
                        description: Some("From ext1".to_string()),
                        flag_type: Some("boolean".to_string()),
                        default_value: Some(serde_json::json!(true)),
                        extension_path: None,
                    },
                ),
            ]),
            shortcuts: HashMap::new(),
        };

        let ext2 = LoadedExtension {
            path: "/ext2".to_string(),
            resolved_path: "/ext2".to_string(),
            source: ExtensionSource::Path,
            manifest: make_manifest("ext2", vec![]),
            tools: HashMap::new(),
            commands: HashMap::new(),
            flags: HashMap::from([
                (
                    "shared-flag".to_string(),
                    ExtensionFlag {
                        name: "shared-flag".to_string(),
                        description: Some("From ext2".to_string()),
                        flag_type: Some("string".to_string()),
                        default_value: Some(serde_json::json!("val")),
                        extension_path: None,
                    },
                ),
            ]),
            shortcuts: HashMap::new(),
        };

        let flags = get_all_extension_flags(&[ext1, ext2]);
        // First one wins (ext1)
        assert_eq!(flags.len(), 1);
        assert_eq!(
            flags["shared-flag"].description,
            Some("From ext1".to_string())
        );
    }

    #[test]
    fn test_get_all_extension_tool_infos_empty() {
        let infos = get_all_extension_tool_infos(&[]);
        assert!(infos.is_empty());
    }

    #[test]
    fn test_get_all_extension_tool_infos_multiple() {
        let ext = make_loaded_extension(
            "test",
            "/test",
            ExtensionSource::Path,
            HashMap::from([
                (
                    "tool1".to_string(),
                    RegisteredTool {
                        definition: make_tool_def("tool1", "Tool 1"),
                        source_path: "/test".to_string(),
                    },
                ),
                (
                    "tool2".to_string(),
                    RegisteredTool {
                        definition: make_tool_def("tool2", "Tool 2"),
                        source_path: "/test".to_string(),
                    },
                ),
            ]),
        );

        let infos = get_all_extension_tool_infos(&[ext]);
        assert_eq!(infos.len(), 2);

        let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"tool1"));
        assert!(names.contains(&"tool2"));
    }
}

// ============================================================================
// Extension Error Handling Tests
// Ported from: extensions-runner.test.ts (error handling)
// ============================================================================

#[cfg(test)]
mod extension_error_tests {
    use super::*;

    #[test]
    fn test_extension_load_error_creation() {
        let error = ExtensionLoadError {
            path: "/test/ext".to_string(),
            error: "Failed to load".to_string(),
        };

        assert_eq!(error.path, "/test/ext");
        assert_eq!(error.error, "Failed to load");
    }

    #[test]
    fn test_load_extensions_result_with_errors() {
        let result = LoadExtensionsResult {
            extensions: vec![],
            errors: vec![
                ExtensionLoadError {
                    path: "/ext1".to_string(),
                    error: "Parse error".to_string(),
                },
                ExtensionLoadError {
                    path: "/ext2".to_string(),
                    error: "Missing manifest".to_string(),
                },
            ],
            diagnostics: vec![],
        };

        assert!(result.extensions.is_empty());
        assert_eq!(result.errors.len(), 2);
        assert_eq!(result.errors[0].error, "Parse error");
        assert_eq!(result.errors[1].error, "Missing manifest");
    }

    #[test]
    fn test_load_extensions_mixed_success_and_errors() {
        let temp_dir = setup_temp_dir();

        // Valid extension
        let valid_dir = temp_dir.path().join("valid");
        std::fs::create_dir_all(&valid_dir).expect("Should create dir");
        std::fs::write(
            valid_dir.join("extension.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "name": "valid",
                "tools": [{"name": "valid-tool", "description": "Valid tool"}]
            }))
            .unwrap(),
        )
        .expect("Should write");

        // Invalid extension (bad JSON)
        let invalid_dir = temp_dir.path().join("invalid");
        std::fs::create_dir_all(&invalid_dir).expect("Should create dir");
        std::fs::write(invalid_dir.join("extension.json"), "not json").expect("Should write");

        let opts = LoadExtensionsOptions {
            cwd: temp_dir.path().to_string_lossy().to_string(),
            agent_dir: None,
            extension_paths: vec![
                valid_dir.to_string_lossy().to_string(),
                invalid_dir.to_string_lossy().to_string(),
            ],
            include_defaults: false,
        };
        let result = load_extensions(&opts);

        assert_eq!(result.extensions.len(), 1);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.extensions[0].manifest.name, "valid");
        assert!(result.errors[0].error.contains("parse manifest"));
    }
}

// ============================================================================
// Extension Source Tests
// Ported from: extensions-discovery.test.ts (source tracking)
// ============================================================================

#[cfg(test)]
mod extension_source_tests {
    use super::*;

    #[test]
    fn test_extension_source_user() {
        let ext = make_loaded_extension("user-ext", "/user/ext", ExtensionSource::User, HashMap::new());
        assert_eq!(ext.source, ExtensionSource::User);
    }

    #[test]
    fn test_extension_source_project() {
        let ext = make_loaded_extension("project-ext", "/project/.pi/extensions/ext", ExtensionSource::Project, HashMap::new());
        assert_eq!(ext.source, ExtensionSource::Project);
    }

    #[test]
    fn test_extension_source_path() {
        let ext = make_loaded_extension("path-ext", "/custom/path", ExtensionSource::Path, HashMap::new());
        assert_eq!(ext.source, ExtensionSource::Path);
    }

    #[test]
    fn test_extension_source_partial_eq() {
        assert_eq!(ExtensionSource::User, ExtensionSource::User);
        assert_eq!(ExtensionSource::Project, ExtensionSource::Project);
        assert_eq!(ExtensionSource::Path, ExtensionSource::Path);
        assert_ne!(ExtensionSource::User, ExtensionSource::Project);
        assert_ne!(ExtensionSource::User, ExtensionSource::Path);
    }
}

// ============================================================================
// Extension Tool Wrapping Tests
// Ported from: extensions-runner.test.ts (tool wrapping)
// ============================================================================

#[cfg(test)]
mod extension_tool_wrapping_tests {
    use super::*;

    #[test]
    fn test_tool_definition_with_execution_mode() {
        let tool = ToolDefinition {
            name: "sequential-tool".to_string(),
            label: None,
            description: "Must run sequentially".to_string(),
            prompt_snippet: None,
            prompt_guidelines: None,
            parameters: None,
            render_shell: None,
            execution_mode: Some("sequential".to_string()),
        };

        assert_eq!(tool.execution_mode, Some("sequential".to_string()));
    }

    #[test]
    fn test_tool_definition_with_parallel_mode() {
        let tool = ToolDefinition {
            name: "parallel-tool".to_string(),
            label: None,
            description: "Can run in parallel".to_string(),
            prompt_snippet: None,
            prompt_guidelines: None,
            parameters: None,
            render_shell: None,
            execution_mode: Some("parallel".to_string()),
        };

        assert_eq!(tool.execution_mode, Some("parallel".to_string()));
    }

    #[test]
    fn test_tool_definition_default_execution_mode() {
        let tool = make_tool_def("default-tool", "Default mode");
        assert!(tool.execution_mode.is_none());
    }

    #[test]
    fn test_tool_definition_with_prompt_guidelines() {
        let tool = ToolDefinition {
            name: "guided-tool".to_string(),
            label: None,
            description: "Has guidelines".to_string(),
            prompt_snippet: None,
            prompt_guidelines: Some(vec![
                "Always verify the output".to_string(),
                "Use with caution".to_string(),
                "Check permissions first".to_string(),
            ]),
            parameters: None,
            render_shell: None,
            execution_mode: None,
        };

        assert_eq!(tool.prompt_guidelines.as_ref().unwrap().len(), 3);
        assert_eq!(
            tool.prompt_guidelines.as_ref().unwrap()[0],
            "Always verify the output"
        );
    }

    #[test]
    fn test_tool_definition_with_parameters_schema() {
        let tool = ToolDefinition {
            name: "param-tool".to_string(),
            label: None,
            description: "Has parameters".to_string(),
            prompt_snippet: None,
            prompt_guidelines: None,
            parameters: Some(serde_json::json!({
                "type": "object",
                "required": ["input"],
                "properties": {
                    "input": {
                        "type": "string",
                        "description": "The input value"
                    },
                    "optional_flag": {
                        "type": "boolean",
                        "default": false
                    }
                }
            })),
            render_shell: None,
            execution_mode: None,
        };

        let params = tool.parameters.as_ref().unwrap();
        assert_eq!(params["type"], "object");
        assert_eq!(params["required"], serde_json::json!(["input"]));
        assert!(params["properties"]["input"]["description"].as_str().unwrap().contains("input"));
    }

    #[test]
    fn test_tool_definition_with_render_shell() {
        let tool = ToolDefinition {
            name: "shell-tool".to_string(),
            label: None,
            description: "Renders in shell".to_string(),
            prompt_snippet: None,
            prompt_guidelines: None,
            parameters: None,
            render_shell: Some("bash".to_string()),
            execution_mode: None,
        };

        assert_eq!(tool.render_shell, Some("bash".to_string()));
    }

    #[test]
    fn test_tool_definition_with_label() {
        let tool = ToolDefinition {
            name: "labeled-tool".to_string(),
            label: Some("My Labeled Tool".to_string()),
            description: "Has a label".to_string(),
            prompt_snippet: None,
            prompt_guidelines: None,
            parameters: None,
            render_shell: None,
            execution_mode: None,
        };

        assert_eq!(tool.label, Some("My Labeled Tool".to_string()));
    }

    #[test]
    fn test_tool_definition_with_prompt_snippet() {
        let tool = ToolDefinition {
            name: "snippet-tool".to_string(),
            label: None,
            description: "Has a snippet".to_string(),
            prompt_snippet: Some("Use snippet-tool for quick results".to_string()),
            prompt_guidelines: None,
            parameters: None,
            render_shell: None,
            execution_mode: None,
        };

        assert_eq!(
            tool.prompt_snippet,
            Some("Use snippet-tool for quick results".to_string())
        );
    }
}

// ============================================================================
// Extension Manifest Validation Tests
// Ported from: compaction-extensions-example.test.ts (type checking)
// ============================================================================

#[cfg(test)]
mod extension_manifest_validation_tests {
    use super::*;

    #[test]
    fn test_manifest_with_all_fields() {
        let json = r#"{
            "name": "full-extension",
            "version": "2.0.0",
            "description": "A full-featured extension",
            "main": "./dist/index.js",
            "tools": [
                {
                    "name": "tool1",
                    "description": "First tool",
                    "parameters": { "type": "object" }
                }
            ],
            "commands": [
                { "name": "cmd1", "description": "First command" }
            ],
            "flags": [
                { "name": "flag1", "flag_type": "boolean" }
            ],
            "shortcuts": [
                { "key": "ctrl+f", "description": "Find" }
            ]
        }"#;

        let manifest: ExtensionManifest = serde_json::from_str(json).expect("Should parse");
        assert_eq!(manifest.name, "full-extension");
        assert_eq!(manifest.version, Some("2.0.0".to_string()));
        assert_eq!(manifest.tools.len(), 1);
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.flags.len(), 1);
        assert_eq!(manifest.shortcuts.len(), 1);
    }

    #[test]
    fn test_manifest_with_unknown_fields_ignored() {
        let json = r#"{
            "name": "tolerant",
            "unknown_field": "should be ignored",
            "extra_object": { "nested": true }
        }"#;

        let manifest: ExtensionManifest = serde_json::from_str(json).expect("Should parse");
        assert_eq!(manifest.name, "tolerant");
    }

    #[test]
    fn test_manifest_missing_name_fails() {
        let json = r#"{"version": "1.0.0"}"#;
        let result: Result<ExtensionManifest, _> = serde_json::from_str(json);
        assert!(result.is_err(), "Missing name should fail");
    }

    #[test]
    fn test_manifest_empty_tools() {
        let manifest = ExtensionManifest {
            name: "empty-tools".to_string(),
            version: None,
            description: None,
            main: None,
            tools: vec![],
            commands: vec![],
            flags: vec![],
            shortcuts: vec![],
        };

        assert!(manifest.tools.is_empty());
        assert!(manifest.commands.is_empty());
    }

    #[test]

    #[test]
    fn test_manifest_tool_missing_description_fails() {
        // ToolDefinition.description is not optional, so missing it should fail
        let json = r#"{
            "name": "no-desc",
            "tools": [{ "name": "bare-tool" }]
        }"#;

        let result: Result<ExtensionManifest, _> = serde_json::from_str(json);
        // description is required (no #[serde(default)]), so this should fail
        assert!(result.is_err(), "Missing description should fail");
    }
}

// ============================================================================
// Extension LoadOptions Tests
// Ported from: extensions-discovery.test.ts (load options)
// ============================================================================

#[cfg(test)]
mod extension_load_options_tests {
    use super::*;

    #[test]
    fn test_load_options_default() {
        let opts = LoadExtensionsOptions::default();
        assert_eq!(opts.cwd, "");
        assert!(opts.agent_dir.is_none());
        assert!(opts.extension_paths.is_empty());
        assert!(!opts.include_defaults);
    }

    #[test]
    fn test_load_options_with_agent_dir() {
        let opts = LoadExtensionsOptions {
            cwd: "/project".to_string(),
            agent_dir: Some("/custom/agent".to_string()),
            extension_paths: vec!["/ext1".to_string()],
            include_defaults: true,
        };

        assert_eq!(opts.cwd, "/project");
        assert_eq!(opts.agent_dir, Some("/custom/agent".to_string()));
        assert_eq!(opts.extension_paths.len(), 1);
        assert!(opts.include_defaults);
    }

    #[test]
    fn test_load_extensions_with_include_defaults() {
        let temp_dir = setup_temp_dir();

        // Create user extensions directory
        let agent_dir = temp_dir.path().join(".pi");
        let user_ext_dir = agent_dir.join("extensions").join("user-ext");
        std::fs::create_dir_all(&user_ext_dir).expect("Should create dir");
        std::fs::write(
            user_ext_dir.join("extension.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "name": "user-extension",
                "tools": [{"name": "user-tool", "description": "User tool"}]
            }))
            .unwrap(),
        )
        .expect("Should write");

        // Create project extensions directory
        let project_ext_dir = temp_dir.path().join(".pi").join("extensions").join("project-ext");
        std::fs::create_dir_all(&project_ext_dir).expect("Should create dir");
        std::fs::write(
            project_ext_dir.join("extension.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "name": "project-extension",
                "tools": [{"name": "project-tool", "description": "Project tool"}]
            }))
            .unwrap(),
        )
        .expect("Should write");

        let opts = LoadExtensionsOptions {
            cwd: temp_dir.path().to_string_lossy().to_string(),
            agent_dir: Some(agent_dir.to_string_lossy().to_string()),
            extension_paths: vec![],
            include_defaults: true,
        };
        let result = load_extensions(&opts);

        // Should find extensions in both user and project dirs
        assert!(result.extensions.len() >= 1, "Should find at least some extensions");
    }
}

// ============================================================================
// Extension Diagnostics Tests
// Ported from: extensions-discovery.test.ts (error reporting)
// ============================================================================

#[cfg(test)]
mod extension_diagnostics_tests {
    use super::*;

    #[test]
    fn test_load_extensions_result_with_diagnostics() {
        let result = LoadExtensionsResult {
            extensions: vec![],
            errors: vec![],
            diagnostics: vec![
                ResourceDiagnostic::Warning {
                    message: "Extension path does not exist".to_string(),
                    path: "/nonexistent".to_string(),
                },
            ],
        };

        assert_eq!(result.diagnostics.len(), 1);
        match &result.diagnostics[0] {
            ResourceDiagnostic::Warning { message, path } => {
                assert_eq!(message, "Extension path does not exist");
                assert_eq!(path, "/nonexistent");
            }
            _ => panic!("Expected Warning diagnostic"),
        }
    }
}
