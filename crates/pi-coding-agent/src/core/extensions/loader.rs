//! Module loader for the embedded extension runtime.
//!
//! Implements `deno_core::ModuleLoader` to:
//!   - resolve relative/file specifiers for extension imports
//!   - load + transpile `.ts`/`.tsx` files via `deno_ast` (swc), pass `.js` through
//! The `ext:pi_extension/runtime.js` entry is served by the `extension!` macro's
//! own built-in loader; this loader only handles user extension files.
//!
//! Also provides `discover_extensions`: scans project-local, global, and explicit
//! paths (ported from the former `rpc-host/src/index.ts::discoverExtensions`).

use std::path::{Path, PathBuf};

use deno_ast::{MediaType, ParseParams, SourceMapOption, TranspileOptions};
use deno_core::{
    error::ModuleLoaderError, ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse,
    ModuleSource, ModuleSourceCode, ModuleSpecifier, ModuleType, ResolutionKind,
};
use deno_error::JsErrorBox;

// ============================================================================
// TsModuleLoader
// ============================================================================

pub struct TsModuleLoader;

impl TsModuleLoader {
    pub fn new() -> Self {
        Self
    }
}

impl deno_core::ModuleLoader for TsModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
        deno_core::resolve_import(specifier, referrer)
            .map_err(|e| JsErrorBox::generic(e.to_string()))
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        // Only handle file: specifiers (extension files on disk).
        // ext:pi_extension/* is served by the extension! macro's built-in loader.
        if module_specifier.scheme() != "file" {
            return ModuleLoadResponse::Sync(Err(
                JsErrorBox::generic(format!(
                    "Unsupported module scheme: {}",
                    module_specifier
                )),
            ));
        }

        let path = match module_specifier.to_file_path() {
            Ok(p) => p,
            Err(_) => {
                return ModuleLoadResponse::Sync(Err(
                    JsErrorBox::generic(format!(
                        "Invalid file path: {}",
                        module_specifier
                    )),
                ))
            }
        };

        let media_type = MediaType::from_path(&path);
        match std::fs::read_to_string(&path) {
            Ok(code) => {
                let code = if media_type.is_declaration() || needs_transpile(media_type) {
                    match transpile(&code, &path, media_type, module_specifier) {
                        Ok(t) => t,
                        Err(e) => {
                            return ModuleLoadResponse::Sync(Err(
                                JsErrorBox::generic(format!(
                                    "Transpile error for {}: {}",
                                    path.display(),
                                    e
                                )),
                            ))
                        }
                    }
                } else {
                    code
                };
                let module_source = ModuleSource::new(
                    ModuleType::JavaScript,
                    ModuleSourceCode::String(code.into()),
                    module_specifier,
                    None,
                );
                ModuleLoadResponse::Sync(Ok(module_source))
            }
            Err(e) => ModuleLoadResponse::Sync(Err(
                JsErrorBox::generic(format!(
                    "Error reading {}: {}",
                    path.display(),
                    e
                )),
            )),
        }
    }
}

fn needs_transpile(media_type: MediaType) -> bool {
    matches!(
        media_type,
        MediaType::TypeScript | MediaType::Tsx | MediaType::Mts | MediaType::Cts | MediaType::Dts
    )
}

fn transpile(
    code: &str,
    path: &Path,
    media_type: MediaType,
    specifier: &ModuleSpecifier,
) -> Result<String, String> {
    let parsed = deno_ast::parse_module(ParseParams {
        specifier: specifier.clone(),
        text: code.into(),
        media_type,
        capture_tokens: false,
        scope_analysis: false,
        maybe_syntax: None,
    })
    .map_err(|e| e.to_string())?;

    let res = parsed
        .transpile(
            &TranspileOptions {
                imports_not_used_as_values: deno_ast::ImportsNotUsedAsValues::Remove,
                ..Default::default()
            },
            &deno_ast::TranspileModuleOptions { module_kind: None },
            &deno_ast::EmitOptions {
                source_map: SourceMapOption::None,
                inline_sources: false,
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;

    Ok(res.into_source().text)
}

// ============================================================================
// Discovery (ported from rpc-host/src/index.ts::discoverExtensions)
// ============================================================================

/// A discovered extension entrypoint (a single .ts/.js file or index inside a dir).
#[derive(Debug, Clone)]
pub struct DiscoveredExtension {
    pub path: PathBuf,
}

/// Discover extension entrypoints from project-local, global, and explicit paths.
///
/// Order: project-local `{cwd}/.pi-rs/extensions/`, global `{agent_dir}/extensions/`,
/// then explicit `paths`. Deduped by resolved path.
pub fn discover_extensions(
    cwd: &str,
    agent_dir: Option<&str>,
    explicit_paths: &[String],
) -> Vec<DiscoveredExtension> {
    let mut out: Vec<DiscoveredExtension> = Vec::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    let add = |p: PathBuf, out: &mut Vec<DiscoveredExtension>, seen: &mut std::collections::HashSet<PathBuf>| {
        if let Ok(canon) = p.canonicalize() {
            if seen.insert(canon) {
                out.push(DiscoveredExtension { path: p });
            }
        } else if seen.insert(p.clone()) {
            out.push(DiscoveredExtension { path: p });
        }
    };

    // 1. Project-local: {cwd}/.pi-rs/extensions/
    let project_ext_dir = Path::new(cwd).join(".pi-rs").join("extensions");
    for ext in discover_in_dir(&project_ext_dir) {
        add(ext, &mut out, &mut seen);
    }

    // 2. Global: {agent_dir}/extensions/
    if let Some(agent) = agent_dir {
        let global_ext_dir = Path::new(agent).join("extensions");
        for ext in discover_in_dir(&global_ext_dir) {
            add(ext, &mut out, &mut seen);
        }
    }

    // 3. Explicit paths
    for raw in explicit_paths {
        let p = Path::new(raw);
        if !p.exists() {
            continue;
        }
        if p.is_file() {
            add(p.to_path_buf(), &mut out, &mut seen);
        } else if p.is_dir() {
            // Directory: look for index.{ts,js} or scan per package.json manifest.
            if let Some(idx) = find_index(p) {
                add(idx, &mut out, &mut seen);
            } else {
                for ext in discover_in_dir(p) {
                    add(ext, &mut out, &mut seen);
                }
            }
        }
    }

    out
}

/// Scan a directory one level: direct .ts/.js files, or subdirectories with
/// `package.json` `pi.extensions` manifest / `index.{ts,js}`.
fn discover_in_dir(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if has_ext(&path, &["ts", "js", "mjs", "cjs", "tsx"]) {
                out.push(path);
            }
        } else if path.is_dir() {
            // package.json pi.extensions manifest?
            let pkg = path.join("package.json");
            if pkg.exists() {
                if let Ok(content) = std::fs::read_to_string(&pkg) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(exts) = v
                            .get("pi")
                            .and_then(|p| p.get("extensions"))
                            .and_then(|e| e.as_array())
                        {
                            for ext in exts {
                                if let Some(s) = ext.as_str() {
                                    let resolved = path.join(s);
                                    if resolved.is_file() {
                                        out.push(resolved);
                                    } else if resolved.is_dir() {
                                        if let Some(idx) = find_index(&resolved) {
                                            out.push(idx);
                                        }
                                    }
                                }
                            }
                            continue;
                        }
                    }
                }
            }
            if let Some(idx) = find_index(&path) {
                out.push(idx);
            }
        }
    }
    out
}

fn find_index(dir: &Path) -> Option<PathBuf> {
    for name in &["index.ts", "index.js", "index.mjs", "index.tsx"] {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn has_ext(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| exts.contains(&e))
        .unwrap_or(false)
}