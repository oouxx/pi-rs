//! Module loader for the embedded extension runtime.
//!
//! Implements `deno_core::ModuleLoader` to:
//!   - resolve relative/file specifiers for extension imports
//!   - resolve bare specifiers via Node.js `node_modules` walk
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

/// A `deno_core::ModuleLoader` that implements Node.js-style module resolution.
///
/// Resolution order:
/// 1. Relative/absolute specifiers (`./`, `../`, `/`, `file://`) → `deno_core::resolve_import`
/// 2. Bare specifiers (e.g. `lodash`, `@scope/name`) → Node.js `node_modules` walk
/// 3. Fallback: check the project's own `node_modules/` (embedded at compile time)
///
/// The `node_modules` walk starts from the referrer file's directory and walks
/// up the directory tree, checking `node_modules/<package>/` at each level.
/// Package entry points are resolved via `package.json` `exports`/`main` fields.
pub struct TsModuleLoader {
    /// Optional fallback path to the project's `node_modules` directory.
    /// Used as a last resort when the standard walk doesn't find the package.
    /// Set at compile time via `env!("CARGO_MANIFEST_DIR")` for development.
    pub(crate) fallback_node_modules: Option<PathBuf>,
}

impl TsModuleLoader {
    pub fn new() -> Self {
        // Embed the path to the project's node_modules at compile time.
        // This is used as a fallback for resolving @earendil-works/* packages
        // that are installed in the project's node_modules during development.
        let fallback = Self::find_project_node_modules();
        Self { fallback_node_modules: fallback }
    }

    /// Walk up from CARGO_MANIFEST_DIR to find the workspace root's node_modules.
    fn find_project_node_modules() -> Option<PathBuf> {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut dir: Option<&Path> = Some(manifest_dir.as_path());
        while let Some(d) = dir {
            let candidate = d.join("node_modules");
            if candidate.is_dir() {
                return Some(candidate);
            }
            dir = d.parent();
        }
        None
    }

    /// Resolve a bare specifier (e.g. `lodash`, `@scope/name`, `lodash/merge`)
    /// by walking up from the referrer file's directory.
    pub(crate) fn resolve_node_modules(
        &self,
        specifier: &str,
        referrer: &str,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
        let (package_name, subpath) = parse_bare_specifier(specifier);

        // Get the referrer's directory.
        let referrer_url = deno_core::resolve_import(referrer, "file:///")
            .map_err(|e| JsErrorBox::generic(format!("Invalid referrer: {e}")))?;
        let referrer_path = referrer_url
            .to_file_path()
            .map_err(|_| JsErrorBox::generic(format!("Referrer is not a file path: {referrer}")))?;
        let mut search_dir = referrer_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("/"));

        // Walk up the directory tree looking for node_modules/<package>/
        loop {
            let nm_dir = search_dir.join("node_modules");
            let pkg_dir = nm_dir.join(&package_name);

            if pkg_dir.is_dir() {
                // Found the package — resolve the entry point.
                let entry = resolve_package_entry(&pkg_dir, &subpath)?;
                let spec = deno_core::resolve_path(
                    &entry.to_string_lossy(),
                    &std::env::current_dir().unwrap_or_default(),
                )
                .map_err(|e| JsErrorBox::generic(format!("Failed to resolve package entry: {e}")))?;
                return Ok(spec);
            }

            // Walk up to parent directory.
            if let Some(parent) = search_dir.parent() {
                search_dir = parent.to_path_buf();
            } else {
                break;
            }
        }

        // Fallback: check the project's own node_modules.
        if let Some(fallback) = &self.fallback_node_modules {
            let pkg_dir = fallback.join(&package_name);
            if pkg_dir.is_dir() {
                let entry = resolve_package_entry(&pkg_dir, &subpath)?;
                let spec = deno_core::resolve_path(
                    &entry.to_string_lossy(),
                    &std::env::current_dir().unwrap_or_default(),
                )
                .map_err(|e| JsErrorBox::generic(format!("Failed to resolve package entry: {e}")))?;
                return Ok(spec);
            }
        }

        Err(JsErrorBox::generic(format!(
            "Cannot find module '{specifier}' — not found in node_modules walk from {referrer}"
        )))
    }
}

impl Default for TsModuleLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl deno_core::ModuleLoader for TsModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
        // 1. Relative/absolute specifiers: delegate to deno_core.
        if specifier.starts_with("./")
            || specifier.starts_with("../")
            || specifier.starts_with('/')
            || specifier.starts_with("file://")
            || specifier.starts_with("data:")
        {
            return deno_core::resolve_import(specifier, referrer)
                .map_err(|e| JsErrorBox::generic(e.to_string()));
        }

        // 2. Bare specifier: Node.js node_modules resolution.
        self.resolve_node_modules(specifier, referrer)
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

// ============================================================================
// Bare specifier parsing
// ============================================================================

/// Parse a bare specifier into (package_name, subpath).
///
/// Examples:
/// - `lodash` → (`"lodash"`, `""`)
/// - `lodash/merge` → (`"lodash"`, `"merge"`)
/// - `@scope/name` → (`"@scope/name"`, `""`)
/// - `@scope/name/sub/path` → (`"@scope/name"`, `"sub/path"`)
fn parse_bare_specifier(specifier: &str) -> (String, String) {
    if specifier.starts_with('@') {
        // Scoped package: @scope/name[/subpath]
        if let Some(rest) = specifier.strip_prefix('@') {
            if let Some(slash_pos) = rest.find('/') {
                let scope_name = &rest[..slash_pos];
                let after_scope = &rest[slash_pos + 1..];
                let package_name = format!("@{scope_name}/{package}", package = {
                    if let Some(next_slash) = after_scope.find('/') {
                        &after_scope[..next_slash]
                    } else {
                        after_scope
                    }
                });
                let subpath = if let Some(next_slash) = after_scope.find('/') {
                    after_scope[next_slash + 1..].to_string()
                } else {
                    String::new()
                };
                return (package_name, subpath);
            }
        }
        // Just @scope with no name — invalid, but handle gracefully.
        (specifier.to_string(), String::new())
    } else {
        // Regular package: name[/subpath]
        if let Some(slash_pos) = specifier.find('/') {
            let package_name = specifier[..slash_pos].to_string();
            let subpath = specifier[slash_pos + 1..].to_string();
            (package_name, subpath)
        } else {
            (specifier.to_string(), String::new())
        }
    }
}

// ============================================================================
// Package entry point resolution
// ============================================================================

/// Resolve the entry point file within a package directory.
///
/// Resolution order:
/// 1. If subpath is non-empty, resolve `<pkg_dir>/<subpath>` (with extension guessing)
/// 2. Check `package.json` `exports` field for the subpath
/// 3. Check `package.json` `main` field
/// 4. Fall back to `index.js` / `index.ts` / `index.mjs`
fn resolve_package_entry(pkg_dir: &Path, subpath: &str) -> Result<PathBuf, ModuleLoaderError> {
    if !subpath.is_empty() {
        // Resolve subpath within the package.
        let subpath_file = pkg_dir.join(subpath);
        if let Some(resolved) = resolve_file_with_extensions(&subpath_file) {
            return Ok(resolved);
        }
        // Try as a directory with index file.
        if subpath_file.is_dir() {
            if let Some(idx) = find_index(&subpath_file) {
                return Ok(idx);
            }
        }
        // Try with .js extension appended (common for subpath imports).
        let with_js = pkg_dir.join(format!("{subpath}.js"));
        if with_js.is_file() {
            return Ok(with_js);
        }
        // Try with /index.js appended.
        let with_index = pkg_dir.join(&subpath).join("index.js");
        if with_index.is_file() {
            return Ok(with_index);
        }
    }

    // Check package.json for exports/main.
    let pkg_json = pkg_dir.join("package.json");
    if pkg_json.is_file() {
        if let Ok(content) = std::fs::read_to_string(&pkg_json) {
            if let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&content) {
                // Check exports field for the subpath.
                if let Some(resolved) = resolve_exports_field(&pkg, subpath, pkg_dir) {
                    return Ok(resolved);
                }
                // Fall back to main field.
                if subpath.is_empty() {
                    if let Some(main) = pkg.get("main").and_then(|m| m.as_str()) {
                        let main_path = pkg_dir.join(main);
                        if let Some(resolved) = resolve_file_with_extensions(&main_path) {
                            return Ok(resolved);
                        }
                        // Try main as-is.
                        if main_path.is_file() {
                            return Ok(main_path);
                        }
                    }
                }
            }
        }
    }

    // Fall back to index files.
    if subpath.is_empty() {
        if let Some(idx) = find_index(pkg_dir) {
            return Ok(idx);
        }
    }

    Err(JsErrorBox::generic(format!(
        "Cannot resolve entry point for package at {} (subpath: '{subpath}')",
        pkg_dir.display()
    )))
}

/// Resolve a subpath against the `exports` field of a package.json.
///
/// Supports:
/// - `"."` for the main entry
/// - `"./subpath"` for subpath exports
/// - `"./subpath/*"` for wildcard subpath exports
/// - Conditional exports with `"import"` condition
fn resolve_exports_field(
    pkg: &serde_json::Value,
    subpath: &str,
    pkg_dir: &Path,
) -> Option<PathBuf> {
    let exports = pkg.get("exports")?;

    // Build the export key: "." for main entry, "./<subpath>" for subpath exports.
    let export_key = if subpath.is_empty() {
        "."
    } else {
        // Ensure the subpath starts with "./" for matching.
        let key = if subpath.starts_with("./") {
            subpath
        } else {
            // Try with "./" prefix.
            let with_prefix = format!("./{subpath}");
            // Also try wildcard match.
            if let Some(resolved) = try_match_wildcard_export(exports, &with_prefix, pkg_dir) {
                return Some(resolved);
            }
            // Try without prefix for subpath exports that use "./" in the key.
            if let Some(resolved) = try_match_exact_export(exports, &with_prefix, pkg_dir) {
                return Some(resolved);
            }
            return None;
        };
        key
    };

    // Try exact match.
    try_match_exact_export(exports, export_key, pkg_dir)
        .or_else(|| try_match_wildcard_export(exports, export_key, pkg_dir))
}

/// Try to match an exact export key.
fn try_match_exact_export(
    exports: &serde_json::Value,
    key: &str,
    pkg_dir: &Path,
) -> Option<PathBuf> {
    match exports {
        serde_json::Value::String(s) => {
            // exports: "./dist/index.js" — shorthand for the "." key
            if key == "." {
                let path = pkg_dir.join(s);
                if path.is_file() {
                    return Some(path);
                }
            }
            None
        }
        serde_json::Value::Object(map) => {
            // Check for the exact key.
            if let Some(entry) = map.get(key) {
                return resolve_export_value(entry, pkg_dir);
            }
            // Check for conditional exports (e.g. "." → { "import": "./dist/index.js" }).
            if key == "." {
                if let Some(import_entry) = map.get("import") {
                    return resolve_export_value(import_entry, pkg_dir);
                }
                if let Some(default_entry) = map.get("default") {
                    return resolve_export_value(default_entry, pkg_dir);
                }
            }
            None
        }
        _ => None,
    }
}

/// Try to match a wildcard export key (e.g. `"./providers/*"`).
fn try_match_wildcard_export(
    exports: &serde_json::Value,
    key: &str,
    pkg_dir: &Path,
) -> Option<PathBuf> {
    let map = exports.as_object()?;
    for (pattern, value) in map {
        if pattern.ends_with('*') {
            let prefix = pattern.strip_suffix('*')?;
            if key.starts_with(prefix) {
                let suffix = key.strip_prefix(prefix)?;
                if let Some(resolved) = resolve_export_value(value, pkg_dir) {
                    // Replace * in the resolved path with the suffix.
                    let resolved_str = resolved.to_string_lossy();
                    if resolved_str.contains('*') {
                        let final_path = pkg_dir.join(resolved_str.replace('*', suffix));
                        if final_path.is_file() {
                            return Some(final_path);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Resolve an export value (which may be a string or a conditional object).
fn resolve_export_value(value: &serde_json::Value, pkg_dir: &Path) -> Option<PathBuf> {
    match value {
        serde_json::Value::String(s) => {
            let path = pkg_dir.join(s);
            if path.is_file() {
                return Some(path);
            }
            // Try with extension guessing.
            resolve_file_with_extensions(&path)
        }
        serde_json::Value::Object(map) => {
            // Conditional export: try "import" condition first, then "default".
            if let Some(import_val) = map.get("import") {
                if let Some(resolved) = resolve_export_value(import_val, pkg_dir) {
                    return Some(resolved);
                }
            }
            if let Some(default_val) = map.get("default") {
                if let Some(resolved) = resolve_export_value(default_val, pkg_dir) {
                    return Some(resolved);
                }
            }
            None
        }
        _ => None,
    }
}

/// Try to resolve a file path by checking with various extensions.
fn resolve_file_with_extensions(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }

    // Try appending common extensions.
    let path_str = path.to_string_lossy();
    for ext in &["js", "mjs", "cjs", "ts", "tsx", "mts", "cts"] {
        let with_ext = format!("{path_str}.{ext}");
        let p = Path::new(&with_ext);
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }

    None
}

/// Find an index file in a directory.
fn find_index(dir: &Path) -> Option<PathBuf> {
    for name in &["index.js", "index.mjs", "index.cjs", "index.ts", "index.tsx", "index.mts"] {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

// ============================================================================
// Transpilation
// ============================================================================

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
    /// Whether this extension can be hot-reloaded. Extensions loaded via `-e`
    /// (explicit path) are not reloadable; project-local and global extensions are.
    pub reloadable: bool,
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

    let add = |p: PathBuf, reloadable: bool, out: &mut Vec<DiscoveredExtension>, seen: &mut std::collections::HashSet<PathBuf>| {
        if let Ok(canon) = p.canonicalize() {
            if seen.insert(canon) {
                out.push(DiscoveredExtension { path: p, reloadable });
            }
        } else if seen.insert(p.clone()) {
            out.push(DiscoveredExtension { path: p, reloadable });
        }
    };

    // 1. Project-local: {cwd}/.pi-rs/extensions/ (reloadable)
    // NOTE: Uses `.pi-rs` (not `.pi`) to avoid conflicting with the original
    // TypeScript pi's extension directory. This is an intentional deviation.
    let project_ext_dir = Path::new(cwd).join(".pi-rs").join("extensions");
    for ext in discover_in_dir(&project_ext_dir) {
        add(ext, true, &mut out, &mut seen);
    }

    // 2. Global: {agent_dir}/extensions/ (reloadable)
    if let Some(agent) = agent_dir {
        let global_ext_dir = Path::new(agent).join("extensions");
        for ext in discover_in_dir(&global_ext_dir) {
            add(ext, true, &mut out, &mut seen);
        }
    }

    // 3. Explicit paths (NOT reloadable — loaded via `-e` flag)
    for raw in explicit_paths {
        let p = Path::new(raw);
        if !p.exists() {
            continue;
        }
        if p.is_file() {
            add(p.to_path_buf(), false, &mut out, &mut seen);
        } else if p.is_dir() {
            // Directory: look for index.{ts,js} or scan per package.json manifest.
            if let Some(idx) = find_index(p) {
                add(idx, false, &mut out, &mut seen);
            } else {
                for ext in discover_in_dir(p) {
                    add(ext, false, &mut out, &mut seen);
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

fn has_ext(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| exts.contains(&e))
        .unwrap_or(false)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use deno_core::ModuleLoader;
    use std::fs;

    // -----------------------------------------------------------------------
    // parse_bare_specifier tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_bare_specifier_simple() {
        assert_eq!(parse_bare_specifier("lodash"), ("lodash".into(), "".into()));
    }

    #[test]
    fn test_parse_bare_specifier_with_subpath() {
        assert_eq!(
            parse_bare_specifier("lodash/merge"),
            ("lodash".into(), "merge".into())
        );
    }

    #[test]
    fn test_parse_bare_specifier_deep_subpath() {
        assert_eq!(
            parse_bare_specifier("lodash/merge/deep"),
            ("lodash".into(), "merge/deep".into())
        );
    }

    #[test]
    fn test_parse_bare_specifier_scoped() {
        assert_eq!(
            parse_bare_specifier("@scope/name"),
            ("@scope/name".into(), "".into())
        );
    }

    #[test]
    fn test_parse_bare_specifier_scoped_with_subpath() {
        assert_eq!(
            parse_bare_specifier("@scope/name/sub"),
            ("@scope/name".into(), "sub".into())
        );
    }

    #[test]
    fn test_parse_bare_specifier_earendil() {
        assert_eq!(
            parse_bare_specifier("@earendil-works/pi-ai"),
            ("@earendil-works/pi-ai".into(), "".into())
        );
    }

    #[test]
    fn test_parse_bare_specifier_earendil_subpath() {
        assert_eq!(
            parse_bare_specifier("@earendil-works/pi-ai/compat"),
            ("@earendil-works/pi-ai".into(), "compat".into())
        );
    }

    // -----------------------------------------------------------------------
    // resolve_file_with_extensions tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_file_with_extensions_exact() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.js");
        fs::write(&file, "// test").unwrap();

        let resolved = resolve_file_with_extensions(&file);
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap(), file);
    }

    #[test]
    fn test_resolve_file_with_extensions_guess() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.ts");
        fs::write(&file, "// test").unwrap();

        // Try without extension.
        let no_ext = dir.path().join("test");
        let resolved = resolve_file_with_extensions(&no_ext);
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap(), file);
    }

    #[test]
    fn test_resolve_file_with_extensions_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let no_ext = dir.path().join("nonexistent");
        assert!(resolve_file_with_extensions(&no_ext).is_none());
    }

    // -----------------------------------------------------------------------
    // resolve_package_entry tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_package_entry_index_js() {
        let dir = tempfile::tempdir().unwrap();
        let pkg_dir = dir.path().join("node_modules").join("test-pkg");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("index.js"), "export default {};").unwrap();

        let entry = resolve_package_entry(&pkg_dir, "").unwrap();
        assert_eq!(entry, pkg_dir.join("index.js"));
    }

    #[test]
    fn test_resolve_package_entry_main_field() {
        let dir = tempfile::tempdir().unwrap();
        let pkg_dir = dir.path().join("node_modules").join("test-pkg");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(
            pkg_dir.join("package.json"),
            r#"{"name":"test-pkg","main":"./lib/main.js"}"#,
        )
        .unwrap();
        fs::create_dir_all(pkg_dir.join("lib")).unwrap();
        fs::write(pkg_dir.join("lib/main.js"), "export default {};").unwrap();

        let entry = resolve_package_entry(&pkg_dir, "").unwrap();
        assert_eq!(entry, pkg_dir.join("lib/main.js"));
    }

    #[test]
    fn test_resolve_package_entry_exports_field() {
        let dir = tempfile::tempdir().unwrap();
        let pkg_dir = dir.path().join("node_modules").join("test-pkg");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(
            pkg_dir.join("package.json"),
            r#"{"name":"test-pkg","exports":{".":{"import":"./dist/index.js"}}}"#,
        )
        .unwrap();
        fs::create_dir_all(pkg_dir.join("dist")).unwrap();
        fs::write(pkg_dir.join("dist/index.js"), "export default {};").unwrap();

        let entry = resolve_package_entry(&pkg_dir, "").unwrap();
        assert_eq!(entry, pkg_dir.join("dist/index.js"));
    }

    #[test]
    fn test_resolve_package_entry_exports_string() {
        let dir = tempfile::tempdir().unwrap();
        let pkg_dir = dir.path().join("node_modules").join("test-pkg");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(
            pkg_dir.join("package.json"),
            r#"{"name":"test-pkg","exports":"./dist/index.js"}"#,
        )
        .unwrap();
        fs::create_dir_all(pkg_dir.join("dist")).unwrap();
        fs::write(pkg_dir.join("dist/index.js"), "export default {};").unwrap();

        let entry = resolve_package_entry(&pkg_dir, "").unwrap();
        assert_eq!(entry, pkg_dir.join("dist/index.js"));
    }

    #[test]
    fn test_resolve_package_entry_subpath() {
        let dir = tempfile::tempdir().unwrap();
        let pkg_dir = dir.path().join("node_modules").join("test-pkg");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(
            pkg_dir.join("package.json"),
            r#"{"name":"test-pkg","exports":{"./sub":"./dist/sub.js"}}"#,
        )
        .unwrap();
        fs::create_dir_all(pkg_dir.join("dist")).unwrap();
        fs::write(pkg_dir.join("dist/sub.js"), "export default {};").unwrap();

        let entry = resolve_package_entry(&pkg_dir, "sub").unwrap();
        assert_eq!(entry, pkg_dir.join("dist/sub.js"));
    }

    #[test]
    fn test_resolve_package_entry_subpath_direct() {
        let dir = tempfile::tempdir().unwrap();
        let pkg_dir = dir.path().join("node_modules").join("test-pkg");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::create_dir_all(pkg_dir.join("dist")).unwrap();
        fs::write(pkg_dir.join("dist/extra.js"), "export default {};").unwrap();

        // Resolve subpath directly (no package.json exports).
        let entry = resolve_package_entry(&pkg_dir, "dist/extra").unwrap();
        assert_eq!(entry, pkg_dir.join("dist/extra.js"));
    }

    #[test]
    fn test_resolve_package_entry_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let pkg_dir = dir.path().join("node_modules").join("nonexistent-pkg");
        fs::create_dir_all(&pkg_dir).unwrap();

        let result = resolve_package_entry(&pkg_dir, "");
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // resolve_node_modules integration tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_node_modules_walk_up() {
        let root = tempfile::tempdir().unwrap();

        // Create node_modules at root level.
        let nm = root.path().join("node_modules").join("my-pkg");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("index.js"), "export default {};").unwrap();

        // Create a "referrer" file deep in the tree.
        let sub_dir = root.path().join("a").join("b").join("c");
        fs::create_dir_all(&sub_dir).unwrap();
        let referrer_file = sub_dir.join("test.ts");
        fs::write(&referrer_file, "import 'my-pkg';").unwrap();

        let loader = TsModuleLoader::new();
        let referrer = format!("file://{}", referrer_file.display());
        let resolved = loader.resolve_node_modules("my-pkg", &referrer).unwrap();

        let resolved_path = resolved.to_file_path().unwrap();
        assert_eq!(resolved_path, nm.join("index.js"));
    }

    #[test]
    fn test_resolve_node_modules_prefers_nearest() {
        let root = tempfile::tempdir().unwrap();

        // Create node_modules at two levels.
        let nm_root = root.path().join("node_modules").join("my-pkg");
        fs::create_dir_all(&nm_root).unwrap();
        fs::write(nm_root.join("index.js"), "export default {}; // root version").unwrap();

        let sub_dir = root.path().join("sub");
        fs::create_dir_all(&sub_dir).unwrap();
        let nm_sub = sub_dir.join("node_modules").join("my-pkg");
        fs::create_dir_all(&nm_sub).unwrap();
        fs::write(nm_sub.join("index.js"), "export default {}; // sub version").unwrap();

        let referrer_file = sub_dir.join("test.ts");
        fs::write(&referrer_file, "import 'my-pkg';").unwrap();

        let loader = TsModuleLoader::new();
        let referrer = format!("file://{}", referrer_file.display());
        let resolved = loader.resolve_node_modules("my-pkg", &referrer).unwrap();

        let resolved_path = resolved.to_file_path().unwrap();
        assert_eq!(resolved_path, nm_sub.join("index.js"));
    }

    #[test]
    fn test_resolve_node_modules_not_found() {
        let root = tempfile::tempdir().unwrap();
        let referrer_file = root.path().join("test.ts");
        fs::write(&referrer_file, "import 'nonexistent-pkg';").unwrap();

        let loader = TsModuleLoader::new();
        let referrer = format!("file://{}", referrer_file.display());
        let result = loader.resolve_node_modules("nonexistent-pkg", &referrer);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_node_modules_with_subpath() {
        let root = tempfile::tempdir().unwrap();

        let pkg_dir = root.path().join("node_modules").join("my-pkg");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::create_dir_all(pkg_dir.join("dist")).unwrap();
        fs::write(pkg_dir.join("dist/extra.js"), "export default {};").unwrap();

        let referrer_file = root.path().join("test.ts");
        fs::write(&referrer_file, "import 'my-pkg/dist/extra';").unwrap();

        let loader = TsModuleLoader::new();
        let referrer = format!("file://{}", referrer_file.display());
        let resolved = loader.resolve_node_modules("my-pkg/dist/extra", &referrer).unwrap();

        let resolved_path = resolved.to_file_path().unwrap();
        assert_eq!(resolved_path, pkg_dir.join("dist/extra.js"));
    }

    #[test]
    fn test_resolve_node_modules_scoped_package() {
        let root = tempfile::tempdir().unwrap();

        let pkg_dir = root.path().join("node_modules").join("@scope").join("my-pkg");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("index.js"), "export default {};").unwrap();

        let referrer_file = root.path().join("test.ts");
        fs::write(&referrer_file, "import '@scope/my-pkg';").unwrap();

        let loader = TsModuleLoader::new();
        let referrer = format!("file://{}", referrer_file.display());
        let resolved = loader.resolve_node_modules("@scope/my-pkg", &referrer).unwrap();

        let resolved_path = resolved.to_file_path().unwrap();
        assert_eq!(resolved_path, pkg_dir.join("index.js"));
    }

    // -----------------------------------------------------------------------
    // Full resolve() tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_relative_specifier() {
        let loader = TsModuleLoader::new();
        let result = loader.resolve("./foo", "file:///test/bar.js", ResolutionKind::Import);
        assert!(result.is_ok());
        let spec = result.unwrap();
        assert_eq!(spec.path(), "/test/foo");
    }

    #[test]
    fn test_resolve_absolute_specifier() {
        let loader = TsModuleLoader::new();
        let result = loader.resolve("/absolute/path.js", "file:///test/bar.js", ResolutionKind::Import);
        assert!(result.is_ok());
    }

    #[test]
    fn test_resolve_bare_specifier_integration() {
        let root = tempfile::tempdir().unwrap();

        let pkg_dir = root.path().join("node_modules").join("test-lib");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("index.js"), "export default {};").unwrap();

        let referrer_file = root.path().join("ext.ts");
        fs::write(&referrer_file, "import 'test-lib';").unwrap();

        let loader = TsModuleLoader::new();
        let referrer = format!("file://{}", referrer_file.display());
        let result = loader.resolve("test-lib", &referrer, ResolutionKind::Import);
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // @earendil-works resolution via fallback
    // -----------------------------------------------------------------------

    #[test]
    fn test_fallback_node_modules_found() {
        // The fallback is set at compile time from CARGO_MANIFEST_DIR.
        // This test verifies the fallback path exists (it should point to
        // the project's node_modules during development).
        let loader = TsModuleLoader::new();
        assert!(
            loader.fallback_node_modules.is_some(),
            "fallback_node_modules should be set at compile time"
        );

        let fallback = loader.fallback_node_modules.as_ref().unwrap();
        assert!(
            fallback.is_dir(),
            "fallback_node_modules should be a directory: {}",
            fallback.display()
        );
    }

    #[test]
    fn test_resolve_earendil_pi_ai_via_fallback() {
        let root = tempfile::tempdir().unwrap();
        let referrer_file = root.path().join("ext.ts");
        fs::write(&referrer_file, "import '@earendil-works/pi-ai';").unwrap();

        let loader = TsModuleLoader::new();
        let referrer = format!("file://{}", referrer_file.display());
        let result = loader.resolve("@earendil-works/pi-ai", &referrer, ResolutionKind::Import);

        // This should resolve via the fallback to the project's node_modules.
        assert!(result.is_ok(), "Should resolve @earendil-works/pi-ai via fallback: {:?}", result.err());
        let spec = result.unwrap();
        let path = spec.to_file_path().unwrap();
        assert!(
            path.exists(),
            "Resolved path should exist: {}",
            path.display()
        );
        assert!(
            path.to_string_lossy().contains("pi-ai"),
            "Resolved path should contain pi-ai: {}",
            path.display()
        );
    }

    #[test]
    fn test_resolve_earendil_pi_ai_compat_via_fallback() {
        let root = tempfile::tempdir().unwrap();
        let referrer_file = root.path().join("ext.ts");
        fs::write(&referrer_file, "import '@earendil-works/pi-ai/compat';").unwrap();

        let loader = TsModuleLoader::new();
        let referrer = format!("file://{}", referrer_file.display());
        let result = loader.resolve("@earendil-works/pi-ai/compat", &referrer, ResolutionKind::Import);

        assert!(result.is_ok(), "Should resolve @earendil-works/pi-ai/compat via fallback: {:?}", result.err());
        let spec = result.unwrap();
        let path = spec.to_file_path().unwrap();
        assert!(path.exists(), "Resolved path should exist: {}", path.display());
    }

    #[test]
    fn test_resolve_earendil_pi_tui_via_fallback() {
        let root = tempfile::tempdir().unwrap();
        let referrer_file = root.path().join("ext.ts");
        fs::write(&referrer_file, "import '@earendil-works/pi-tui';").unwrap();

        let loader = TsModuleLoader::new();
        let referrer = format!("file://{}", referrer_file.display());
        let result = loader.resolve("@earendil-works/pi-tui", &referrer, ResolutionKind::Import);

        assert!(result.is_ok(), "Should resolve @earendil-works/pi-tui via fallback: {:?}", result.err());
    }

    #[test]
    fn test_resolve_earendil_pi_coding_agent_via_fallback() {
        let root = tempfile::tempdir().unwrap();
        let referrer_file = root.path().join("ext.ts");
        fs::write(&referrer_file, "import '@earendil-works/pi-coding-agent';").unwrap();

        let loader = TsModuleLoader::new();
        let referrer = format!("file://{}", referrer_file.display());
        let result = loader.resolve("@earendil-works/pi-coding-agent", &referrer, ResolutionKind::Import);

        assert!(result.is_ok(), "Should resolve @earendil-works/pi-coding-agent via fallback: {:?}", result.err());
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_data_url() {
        let loader = TsModuleLoader::new();
        let result = loader.resolve(
            "data:text/javascript,export default 42;",
            "file:///test/bar.js",
            ResolutionKind::Import,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_resolve_empty_specifier() {
        let loader = TsModuleLoader::new();
        let result = loader.resolve("", "file:///test/bar.js", ResolutionKind::Import);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_invalid_referrer() {
        let loader = TsModuleLoader::new();
        let result = loader.resolve("lodash", "not-a-url", ResolutionKind::Import);
        assert!(result.is_err());
    }
}
