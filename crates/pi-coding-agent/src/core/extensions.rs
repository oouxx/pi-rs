use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config;
use crate::core::diagnostics::ResourceDiagnostic;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionManifest {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub main: Option<String>,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub commands: Vec<CommandDefinition>,
    #[serde(default)]
    pub flags: Vec<ExtensionFlag>,
    #[serde(default)]
    pub shortcuts: Vec<ExtensionShortcut>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
    #[serde(default)]
    pub prompt_guidelines: Option<Vec<String>>,
    #[serde(default)]
    pub execution_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandDefinition {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionFlag {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default_value: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionShortcut {
    pub key: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ExtensionSource {
    User,
    Project,
    Path,
}

#[derive(Debug, Clone)]
pub struct LoadedExtension {
    pub path: String,
    pub resolved_path: String,
    pub source: ExtensionSource,
    pub manifest: ExtensionManifest,
    pub tools: HashMap<String, RegisteredTool>,
    pub commands: HashMap<String, RegisteredCommand>,
    pub flags: HashMap<String, ExtensionFlag>,
    pub shortcuts: HashMap<String, ExtensionShortcut>,
}

#[derive(Debug, Clone)]
pub struct RegisteredTool {
    pub name: String,
    pub description: String,
    pub parameters: Option<serde_json::Value>,
    pub prompt_guidelines: Vec<String>,
    pub source_path: String,
}

#[derive(Debug, Clone)]
pub struct RegisteredCommand {
    pub name: String,
    pub description: Option<String>,
    pub source_path: String,
}

#[derive(Debug, Clone, Default)]
pub struct LoadExtensionsOptions {
    pub cwd: String,
    pub agent_dir: Option<String>,
    pub extension_paths: Vec<String>,
    pub include_defaults: bool,
}

#[derive(Debug, Clone)]
pub struct LoadExtensionsResult {
    pub extensions: Vec<LoadedExtension>,
    pub errors: Vec<ExtensionLoadError>,
    pub diagnostics: Vec<ResourceDiagnostic>,
}

#[derive(Debug, Clone)]
pub struct ExtensionLoadError {
    pub path: String,
    pub error: String,
}

pub fn load_extensions(options: &LoadExtensionsOptions) -> LoadExtensionsResult {
    let resolved_agent_dir = options
        .agent_dir
        .as_deref()
        .map(|d| d.to_string())
        .unwrap_or_else(|| config::get_agent_dir().to_string_lossy().to_string());

    let mut extensions: Vec<LoadedExtension> = Vec::new();
    let mut errors: Vec<ExtensionLoadError> = Vec::new();
    let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();

    if options.include_defaults {
        let user_ext_dir = Path::new(&resolved_agent_dir).join("extensions");
        if user_ext_dir.exists() {
            load_extensions_from_dir(&user_ext_dir, ExtensionSource::User, &mut extensions, &mut errors);
        }

        let project_ext_dir = Path::new(&options.cwd).join(config::CONFIG_DIR_NAME).join("extensions");
        if project_ext_dir.exists() {
            load_extensions_from_dir(&project_ext_dir, ExtensionSource::Project, &mut extensions, &mut errors);
        }
    }

    for raw_path in &options.extension_paths {
        let path = PathBuf::from(raw_path);
        if !path.exists() {
            diagnostics.push(ResourceDiagnostic::Warning {
                message: "extension path does not exist".to_string(),
                path: raw_path.clone(),
            });
            continue;
        }

        if path.is_dir() {
            load_extension_from_dir(&path, ExtensionSource::Path, &mut extensions, &mut errors);
        }
    }

    LoadExtensionsResult {
        extensions,
        errors,
        diagnostics,
    }
}

fn load_extensions_from_dir(
    dir: &Path,
    source: ExtensionSource,
    extensions: &mut Vec<LoadedExtension>,
    errors: &mut Vec<ExtensionLoadError>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            load_extension_from_dir(&path, source.clone(), extensions, errors);
        }
    }
}

fn load_extension_from_dir(
    dir: &Path,
    source: ExtensionSource,
    extensions: &mut Vec<LoadedExtension>,
    errors: &mut Vec<ExtensionLoadError>,
) {
    let manifest_path = dir.join("extension.json");
    if !manifest_path.exists() {
        let alt_manifest = dir.join("package.json");
        if alt_manifest.exists() {
            load_extension_manifest(&alt_manifest, dir, source, extensions, errors);
        }
        return;
    }
    load_extension_manifest(&manifest_path, dir, source, extensions, errors);
}

fn load_extension_manifest(
    manifest_path: &Path,
    dir: &Path,
    source: ExtensionSource,
    extensions: &mut Vec<LoadedExtension>,
    errors: &mut Vec<ExtensionLoadError>,
) {
    let content = match std::fs::read_to_string(manifest_path) {
        Ok(c) => c,
        Err(e) => {
            errors.push(ExtensionLoadError {
                path: manifest_path.to_string_lossy().to_string(),
                error: format!("Failed to read manifest: {}", e),
            });
            return;
        }
    };

    let manifest: ExtensionManifest = match serde_json::from_str(&content) {
        Ok(m) => m,
        Err(e) => {
            errors.push(ExtensionLoadError {
                path: manifest_path.to_string_lossy().to_string(),
                error: format!("Failed to parse manifest: {}", e),
            });
            return;
        }
    };

    let dir_str = dir.to_string_lossy().to_string();
    let mut tools_map: HashMap<String, RegisteredTool> = HashMap::new();
    let mut commands_map: HashMap<String, RegisteredCommand> = HashMap::new();
    let mut flags_map: HashMap<String, ExtensionFlag> = HashMap::new();
    let mut shortcuts_map: HashMap<String, ExtensionShortcut> = HashMap::new();

    for tool_def in &manifest.tools {
        tools_map.insert(
            tool_def.name.clone(),
            RegisteredTool {
                name: tool_def.name.clone(),
                description: tool_def.description.clone(),
                parameters: tool_def.parameters.clone(),
                prompt_guidelines: tool_def.prompt_guidelines.clone().unwrap_or_default(),
                source_path: dir_str.clone(),
            },
        );
    }

    for cmd_def in &manifest.commands {
        commands_map.insert(
            cmd_def.name.clone(),
            RegisteredCommand {
                name: cmd_def.name.clone(),
                description: cmd_def.description.clone(),
                source_path: dir_str.clone(),
            },
        );
    }

    for flag in &manifest.flags {
        flags_map.insert(flag.name.clone(), flag.clone());
    }

    for shortcut in &manifest.shortcuts {
        shortcuts_map.insert(shortcut.key.clone(), shortcut.clone());
    }

    extensions.push(LoadedExtension {
        path: dir_str.clone(),
        resolved_path: dir_str,
        source,
        manifest,
        tools: tools_map,
        commands: commands_map,
        flags: flags_map,
        shortcuts: shortcuts_map,
    });
}

pub fn get_all_extension_tools(extensions: &[LoadedExtension]) -> Vec<RegisteredTool> {
    let mut tools: Vec<RegisteredTool> = Vec::new();
    for ext in extensions {
        for tool in ext.tools.values() {
            tools.push(tool.clone());
        }
    }
    tools
}

pub fn get_all_extension_commands(extensions: &[LoadedExtension]) -> Vec<RegisteredCommand> {
    let mut commands: Vec<RegisteredCommand> = Vec::new();
    for ext in extensions {
        for cmd in ext.commands.values() {
            commands.push(cmd.clone());
        }
    }
    commands
}

pub fn get_all_extension_flags(extensions: &[LoadedExtension]) -> HashMap<String, ExtensionFlag> {
    let mut flags: HashMap<String, ExtensionFlag> = HashMap::new();
    for ext in extensions {
        for (name, flag) in &ext.flags {
            flags.entry(name.clone()).or_insert_with(|| flag.clone());
        }
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_extensions_empty() {
        let opts = LoadExtensionsOptions {
            cwd: "/nonexistent".to_string(),
            include_defaults: false,
            ..Default::default()
        };
        let result = load_extensions(&opts);
        assert!(result.extensions.is_empty());
    }

    #[test]
    fn test_get_all_extension_tools() {
        let ext = LoadedExtension {
            path: "/test".to_string(),
            resolved_path: "/test".to_string(),
            source: ExtensionSource::Path,
            manifest: ExtensionManifest {
                name: "test".to_string(),
                version: None,
                description: None,
                main: None,
                tools: vec![],
                commands: vec![],
                flags: vec![],
                shortcuts: vec![],
            },
            tools: {
                let mut m = HashMap::new();
                m.insert(
                    "my-tool".to_string(),
                    RegisteredTool {
                        name: "my-tool".to_string(),
                        description: "A test tool".to_string(),
                        parameters: None,
                        prompt_guidelines: vec![],
                        source_path: "/test".to_string(),
                    },
                );
                m
            },
            commands: HashMap::new(),
            flags: HashMap::new(),
            shortcuts: HashMap::new(),
        };
        let tools = get_all_extension_tools(&[ext]);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "my-tool");
    }
}