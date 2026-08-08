//! Extension discovery + module cache — the runtime-agnostic half of TS
//! `core/extensions/loader.ts`.
//!
//! This module ports the pieces of `loader.ts` that need **no JavaScript
//! runtime**: manifest parsing, entry-point resolution, directory scanning,
//! and the cwd-invalidating module cache. The factory-invocation half
//! (`loadExtensionModule` / `loadExtension` / `createExtensionAPI`) requires
//! an embedded JS runtime and is deliberately *not* stubbed here — the
//! factory-invocation half lives in `bun/` (方案 A, Bun subprocess). Anything
//! in this file is reusable regardless of which runtime backend is chosen.
//!
//! TS source: `packages/coding-agent/src/core/extensions/loader.ts`.
//! The functions below mirror the named TS functions 1:1 in behavior; error
//! handling intentionally matches TS (read/parse failures return `None` /
//! empty, *not* `Result::Err`) because the TS original `try/catch`es and
//! returns `null`/`[]` — this is faithful porting, not silent fallback.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::CONFIG_DIR_NAME;
use crate::utils::paths::{resolve_path, PathOptions};

// ============================================================================
// PiManifest — package.json "pi" object
// ============================================================================

/// The `"pi"` object inside a `package.json`. Mirrors TS `PiManifest`.
///
/// All fields are optional; a manifest with no `extensions` simply yields an
/// empty vector (TS checks `manifest?.extensions?.length`).
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct PiManifest {
    /// Entry points declared under `pi.extensions`. Resolved relative to the
    /// package directory.
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub themes: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub prompts: Vec<String>,
}

/// Read and parse the `"pi"` object from a `package.json` file.
///
/// Mirrors TS `readPiManifest(packageJsonPath)`: returns `None` if the file
/// can't be read, isn't valid JSON, or has no `"pi"` object — the TS original
/// wraps everything in `try/catch` returning `null`.
pub fn read_pi_manifest(package_json_path: &Path) -> Option<PiManifest> {
    let content = std::fs::read_to_string(package_json_path).ok()?;
    let pkg: serde_json::Value = serde_json::from_str(&content).ok()?;
    let pi = pkg.get("pi")?;
    if !pi.is_object() {
        return None;
    }
    serde_json::from_value::<PiManifest>(pi.clone()).ok()
}

// ============================================================================
// Entry-point resolution
// ============================================================================

/// Whether `name` looks like an extension source file. Mirrors TS
/// `isExtensionFile(name)`: `.ts` or `.js` suffix.
#[must_use]
pub fn is_extension_file(name: &str) -> bool {
    name.ends_with(".ts") || name.ends_with(".js")
}

/// Resolve extension entry points from a directory.
///
/// Mirrors TS `resolveExtensionEntries(dir)`:
/// 1. If `dir/package.json` exists and its `pi.extensions` lists existing
///    files, return those resolved paths.
/// 2. Otherwise, if `dir/index.ts` exists return `[it]`; else if
///    `dir/index.js` exists return `[it]`.
/// 3. Return `None` if no entry points are found.
///
/// Like the TS version, manifest entries that point at non-existent files are
/// silently skipped; if *all* declared entries are missing, the function
/// falls through to the `index.ts`/`index.js` check (matching TS, which only
/// returns early when `entries.length > 0`).
pub fn resolve_extension_entries(dir: &Path) -> Option<Vec<PathBuf>> {
    // 1. package.json with pi.extensions
    let package_json_path = dir.join("package.json");
    if package_json_path.is_file() {
        if let Some(manifest) = read_pi_manifest(&package_json_path) {
            if !manifest.extensions.is_empty() {
                let entries: Vec<PathBuf> = manifest
                    .extensions
                    .iter()
                    .map(|ext| dir.join(ext))
                    .filter(|p| p.exists())
                    .collect();
                if !entries.is_empty() {
                    return Some(entries);
                }
            }
        }
    }

    // 2. index.ts / index.js
    let index_ts = dir.join("index.ts");
    if index_ts.exists() {
        return Some(vec![index_ts]);
    }
    let index_js = dir.join("index.js");
    if index_js.exists() {
        return Some(vec![index_js]);
    }

    None
}

/// Discover extension entry points within a single directory (one level deep).
///
/// Mirrors TS `discoverExtensionsInDir(dir)`. Discovery rules:
/// 1. Direct files: `*.ts` / `*.js` → load.
/// 2. Subdirectory with `index.ts`/`index.js` → load.
/// 3. Subdirectory with `package.json` `pi.extensions` → load what it declares.
///
/// No recursion beyond one level. Returns an empty vector if the directory
/// doesn't exist or can't be read (matching TS, which `try/catch`es `readdir`
/// and returns `[]`). Symlinks are followed for both the file and directory
/// cases (TS checks `isFile() || isSymbolicLink()` and
/// `isDirectory() || isSymbolicLink()`); Rust's `fs::read_dir` does not
/// report link-ness portably, so we fall back to a metadata check per entry.
pub fn discover_extensions_in_dir(dir: &Path) -> Vec<PathBuf> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut discovered: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let entry_path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        // `entry.file_type()` reports the link itself for symlinks (no follow),
        // so a symlink-to-dir would be classified as neither dir nor file and
        // silently skipped. TS checks `isFile() || isSymbolicLink()` and
        // `isDirectory() || isSymbolicLink()`, i.e. a symlink is classified by
        // what it points at — follow it via `metadata()` when `file_type()`
        // reports a symlink.
        let ftype = match entry.file_type().ok() {
            Some(t) if t.is_symlink() => {
                std::fs::metadata(&entry_path).ok().map(|m| m.file_type())
            }
            other => other,
        };

        let is_dir = ftype.as_ref().is_some_and(|t| t.is_dir());
        let is_file = ftype.as_ref().is_some_and(|t| t.is_file());

        // 1. Direct files: *.ts or *.js
        if is_file && is_extension_file(&name) {
            discovered.push(entry_path);
            continue;
        }

        // 2 & 3. Subdirectories
        if is_dir {
            if let Some(entries) = resolve_extension_entries(&entry_path) {
                discovered.extend(entries);
            }
        }
    }

    discovered
}

// ============================================================================
// discover_extension_paths — the path-collection half of
// discoverAndLoadExtensions (the load half needs a JS runtime)
// ============================================================================

/// Result of extension path discovery. The `paths` are deduplicated, absolute
/// entry points ready to be handed to a (future) module loader; `resolved_cwd`
/// is the canonicalized working directory the loader should use (mirrors TS
/// `resolvedCwd = cacheToken?.cwd ?? resolvePath(cwd)`).
#[derive(Debug, Clone, Default)]
pub struct DiscoveredExtensions {
    /// Deduplicated, absolute entry-point paths (`.ts`/`.js` files).
    pub paths: Vec<PathBuf>,
    /// The resolved working directory, for the loader to inherit.
    pub resolved_cwd: PathBuf,
}

/// Discover extension entry points from all standard locations.
///
/// Mirrors the path-collection portion of TS
/// `discoverAndLoadExtensions(configuredPaths, cwd, agentDir)`. The actual
/// module loading (jiti import + factory call) is runtime-dependent and lives in
/// a future chunk; this function only resolves *which files* to load.
///
/// Discovery order (matches TS):
/// 1. Project-local: `cwd/${CONFIG_DIR_NAME}/extensions/`
/// 2. Global: `agentDir/extensions/`
/// 3. Explicitly configured paths (each resolved against `cwd` with unicode
///    space normalization; directories are scanned, files are kept as-is).
///
/// Paths are deduplicated by their absolute lexical form (TS dedups on
/// `path.resolve(p)`); the first occurrence wins.
pub fn discover_extension_paths(
    configured_paths: &[String],
    cwd: &str,
    agent_dir: &str,
) -> DiscoveredExtensions {
    let normalize_opts = PathOptions {
        normalize_unicode_spaces: true,
        ..PathOptions::default()
    };
    let resolved_cwd = PathBuf::from(resolve_path(cwd, cwd, &PathOptions::default()));
    let resolved_agent_dir = PathBuf::from(resolve_path(
        agent_dir,
        agent_dir,
        &PathOptions::default(),
    ));

    let mut all_paths: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    // 去重键用 canonicalize 后的路径：`pi install` 会把扩展 symlink 进
    // `agent_dir/extensions/`，同一扩展会以 symlink 路径 + 真实路径各出现
    // 一次，词法去重漏掉。canonicalize 解析 symlink 后两者相等。
    // 不存在的路径（保留用于报错）canonicalize 失败，退回词法路径。
    let dedup_key = |p: &PathBuf| -> PathBuf {
        std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())
    };

    let add_paths = |paths: &[PathBuf], all: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>| {
        for p in paths {
            if seen.insert(dedup_key(p)) {
                all.push(p.clone());
            }
        }
    };

    // 1. Project-local extensions: cwd/${CONFIG_DIR_NAME}/extensions/
    let local_ext_dir = resolved_cwd.join(CONFIG_DIR_NAME).join("extensions");
    let local = discover_extensions_in_dir(&local_ext_dir);
    add_paths(&local, &mut all_paths, &mut seen);

    // 2. Global extensions: agentDir/extensions/
    let global_ext_dir = resolved_agent_dir.join("extensions");
    let global = discover_extensions_in_dir(&global_ext_dir);
    add_paths(&global, &mut all_paths, &mut seen);

    // 3. Explicitly configured paths
    for p in configured_paths {
        let resolved = PathBuf::from(resolve_path(p, &resolved_cwd.to_string_lossy(), &normalize_opts));
        if resolved.is_dir() {
            // Directory: check for package.json manifest / index, else scan.
            if let Some(entries) = resolve_extension_entries(&resolved) {
                add_paths(&entries, &mut all_paths, &mut seen);
                continue;
            }
            let discovered = discover_extensions_in_dir(&resolved);
            add_paths(&discovered, &mut all_paths, &mut seen);
            continue;
        }
        // File (or non-existent): keep the resolved path as-is.
        add_paths(std::slice::from_ref(&resolved), &mut all_paths, &mut seen);
    }

    DiscoveredExtensions {
        paths: all_paths,
        resolved_cwd,
    }
}

// ============================================================================
// ExtensionCache — cwd-invalidating module cache (runtime-agnostic structure)
// ============================================================================

/// A snapshot of the cache's cwd + generation at the time a load batch began.
///
/// Mirrors TS `ExtensionCacheToken`. A token is "current" only while the cache
/// cwd is unchanged and no `clear()` has bumped the generation since.
#[derive(Debug, Clone)]
pub struct CacheToken {
    cwd: PathBuf,
    generation: u64,
}

/// Cwd-invalidating cache for loaded extension modules.
///
/// Mirrors the TS module-level `extensionCache` / `extensionCacheCwd` /
/// `extensionCacheGeneration` triple. Generic over the cached value (`V`)
/// so the invalidation logic is reusable regardless of the runtime backend:
/// a future runtime chunk instantiates `ExtensionCache<ExtensionFactory>`,
/// while tests can use `ExtensionCache<()>`.
///
/// Invalidation rules (matching TS):
/// - `use_cwd(cwd)` changes the active cwd; if it differs from the previous
///   one, the cache is cleared and the generation increments.
/// - `clear()` empties the cache, unsets the cwd, and increments the
///   generation (so outstanding tokens become stale).
/// - `get`/`insert` are no-ops (returning `None` / doing nothing) when the
///   supplied token is no longer current — this mirrors TS
///   `isCurrentCacheToken(cacheToken)` guards around cache reads/writes.
pub struct ExtensionCache<V> {
    cache: HashMap<PathBuf, V>,
    cwd: Option<PathBuf>,
    generation: u64,
}

impl<V> Default for ExtensionCache<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V> ExtensionCache<V> {
    /// Create an empty cache with no active cwd (generation 0).
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            cwd: None,
            generation: 0,
        }
    }

    /// Number of currently cached modules.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Whether the cache holds no modules.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Empty the cache, unset the active cwd, and bump the generation.
    ///
    /// Mirrors TS `clearExtensionCache()`. Any outstanding `CacheToken`s
    /// become stale because the generation no longer matches.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.cwd = None;
        self.generation = self.generation.wrapping_add(1);
    }

    /// Set the active cwd for a load batch, returning a token that snapshots
    /// the current (cwd, generation). If the cwd changed since the last call,
    /// the cache is cleared first. Mirrors TS `useExtensionCacheCwd(cwd)`.
    pub fn use_cwd(&mut self, cwd: PathBuf) -> CacheToken {
        let needs_clear = self.cwd.as_ref().is_some_and(|c| c != &cwd);
        if needs_clear {
            self.clear();
        }
        self.cwd = Some(cwd.clone());
        CacheToken {
            cwd,
            generation: self.generation,
        }
    }

    /// Whether `token` still matches the current (cwd, generation).
    /// Mirrors TS `isCurrentCacheToken(cacheToken)`.
    #[must_use]
    pub fn is_current(&self, token: &CacheToken) -> bool {
        self.cwd.as_ref() == Some(&token.cwd) && self.generation == token.generation
    }

    /// Look up a cached module, but only if `token` is still current.
    /// Returns `None` if the token is stale (mirrors TS's
    /// `isCurrentCacheToken` guard before `extensionCache.get`).
    #[must_use]
    pub fn get(&self, token: &CacheToken, path: &Path) -> Option<&V> {
        if self.is_current(token) {
            self.cache.get(path)
        } else {
            None
        }
    }

    /// Insert a loaded module, but only if `token` is still current.
    /// A no-op when the token is stale (mirrors TS's
    /// `isCurrentCacheToken` guard before `extensionCache.set`).
    pub fn insert(&mut self, token: &CacheToken, path: PathBuf, value: V) {
        if self.is_current(token) {
            self.cache.insert(path, value);
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    /// Helper: create a temp dir with given files (relative paths + content).
    fn make_temp_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (rel, content) in files {
            let path = dir.path().join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("mkdir");
            }
            let mut f = fs::File::create(&path).expect("create");
            f.write_all(content.as_bytes()).expect("write");
        }
        dir
    }

    // ---- read_pi_manifest ----

    #[test]
    fn test_read_pi_manifest_with_extensions() {
        let dir = make_temp_dir(&[(
            "package.json",
            r#"{"name":"x","pi":{"extensions":["src/a.ts","src/b.ts"]}}"#,
        )]);
        let m = read_pi_manifest(&dir.path().join("package.json")).expect("manifest");
        assert_eq!(m.extensions, vec!["src/a.ts", "src/b.ts"]);
        assert!(m.themes.is_empty());
    }

    #[test]
    fn test_read_pi_manifest_without_pi_field() {
        let dir = make_temp_dir(&[("package.json", r#"{"name":"x"}"#)]);
        assert!(read_pi_manifest(&dir.path().join("package.json")).is_none());
    }

    #[test]
    fn test_read_pi_manifest_missing_file() {
        let dir = make_temp_dir(&[]);
        assert!(read_pi_manifest(&dir.path().join("package.json")).is_none());
    }

    #[test]
    fn test_read_pi_manifest_invalid_json() {
        let dir = make_temp_dir(&[("package.json", "{not json")]);
        assert!(read_pi_manifest(&dir.path().join("package.json")).is_none());
    }

    #[test]
    fn test_read_pi_manifest_pi_not_object() {
        let dir = make_temp_dir(&[("package.json", r#"{"pi":"nope"}"#)]);
        assert!(read_pi_manifest(&dir.path().join("package.json")).is_none());
    }

    // ---- is_extension_file ----

    #[test]
    fn test_is_extension_file() {
        assert!(is_extension_file("a.ts"));
        assert!(is_extension_file("a.js"));
        assert!(!is_extension_file("a.txt"));
        assert!(!is_extension_file("ats"));
        assert!(!is_extension_file(""));
    }

    // ---- resolve_extension_entries ----

    #[test]
    fn test_resolve_entries_via_manifest() {
        let dir = make_temp_dir(&[
            ("package.json", r#"{"pi":{"extensions":["entry.ts"]}}"#),
            ("entry.ts", "// ext"),
        ]);
        let entries = resolve_extension_entries(dir.path()).expect("entries");
        assert_eq!(entries, vec![dir.path().join("entry.ts")]);
    }

    #[test]
    fn test_resolve_entries_manifest_skips_missing_files_then_falls_to_index() {
        // manifest declares a non-existent file; TS falls through to index check
        let dir = make_temp_dir(&[
            ("package.json", r#"{"pi":{"extensions":["nope.ts"]}}"#),
            ("index.ts", "// idx"),
        ]);
        let entries = resolve_extension_entries(dir.path()).expect("entries");
        assert_eq!(entries, vec![dir.path().join("index.ts")]);
    }

    #[test]
    fn test_resolve_entries_index_ts_preferred_over_index_js() {
        let dir = make_temp_dir(&[("index.ts", "// ts"), ("index.js", "// js")]);
        let entries = resolve_extension_entries(dir.path()).expect("entries");
        assert_eq!(entries, vec![dir.path().join("index.ts")]);
    }

    #[test]
    fn test_resolve_entries_index_js_only() {
        let dir = make_temp_dir(&[("index.js", "// js")]);
        let entries = resolve_extension_entries(dir.path()).expect("entries");
        assert_eq!(entries, vec![dir.path().join("index.js")]);
    }

    #[test]
    fn test_resolve_entries_none() {
        let dir = make_temp_dir(&[("readme.md", "hi")]);
        assert!(resolve_extension_entries(dir.path()).is_none());
    }

    // ---- discover_extensions_in_dir ----

    #[test]
    fn test_discover_direct_files() {
        let dir = make_temp_dir(&[("a.ts", ""), ("b.js", ""), ("c.txt", "")]);
        let mut found = discover_extensions_in_dir(dir.path());
        found.sort();
        let mut expected = vec![dir.path().join("a.ts"), dir.path().join("b.js")];
        expected.sort();
        assert_eq!(found, expected);
    }

    #[test]
    fn test_discover_subdir_with_index() {
        let dir = make_temp_dir(&[("pkg/index.ts", ""), ("pkg/package.json", "{}")]);
        let found = discover_extensions_in_dir(dir.path());
        assert_eq!(found, vec![dir.path().join("pkg").join("index.ts")]);
    }

    #[test]
    fn test_discover_subdir_with_manifest() {
        let dir = make_temp_dir(&[
            ("pkg/package.json", r#"{"pi":{"extensions":["src/main.ts"]}}"#),
            ("pkg/src/main.ts", ""),
        ]);
        let found = discover_extensions_in_dir(dir.path());
        assert_eq!(found, vec![dir.path().join("pkg").join("src").join("main.ts")]);
    }

    #[test]
    fn test_discover_nonexistent_dir_is_empty() {
        assert!(discover_extensions_in_dir(Path::new("/nonexistent/xyzzy")).is_empty());
    }

    #[test]
    fn test_discover_no_recursion_beyond_one_level() {
        // nested pkg/sub should NOT be scanned (no index/manifest at pkg/sub)
        let dir = make_temp_dir(&[("pkg/sub/deep.ts", "")]);
        let found = discover_extensions_in_dir(dir.path());
        assert!(found.is_empty(), "found {found:?}");
    }

    #[cfg(unix)]
    #[test]
    fn test_discover_symlinked_subdir_with_index() {
        // `pi install` symlinks installed packages into the extensions dir;
        // a symlink-to-dir must be discovered like a real dir (TS checks
        // `isDirectory() || isSymbolicLink()`). Regression test: the old code
        // classified symlinks as neither dir nor file and skipped them.
        let target = make_temp_dir(&[("pkg/index.ts", "")]);
        let ext_dir = make_temp_dir(&[]);
        let link = ext_dir.path().join("installed.pkg");
        std::os::unix::fs::symlink(target.path().join("pkg"), &link).expect("symlink");
        let found = discover_extensions_in_dir(ext_dir.path());
        assert_eq!(found, vec![link.join("index.ts")], "found {found:?}");
    }

    #[cfg(unix)]
    #[test]
    fn test_discover_symlinked_ts_file() {
        // A symlink-to-file with a .ts name must be discovered (TS checks
        // `isFile() || isSymbolicLink()`).
        let target = make_temp_dir(&[("real.ts", "")]);
        let ext_dir = make_temp_dir(&[]);
        let link = ext_dir.path().join("linked.ts");
        std::os::unix::fs::symlink(target.path().join("real.ts"), &link).expect("symlink");
        let found = discover_extensions_in_dir(ext_dir.path());
        assert_eq!(found, vec![link], "found {found:?}");
    }

    #[cfg(unix)]
    #[test]
    fn test_discover_symlink_to_nonexistent_target_skipped() {
        // A dangling symlink (target removed) must be skipped, not crash.
        let ext_dir = make_temp_dir(&[]);
        let link = ext_dir.path().join("dangling.pkg");
        std::os::unix::fs::symlink("/nonexistent/target", &link).expect("symlink");
        let found = discover_extensions_in_dir(ext_dir.path());
        assert!(found.is_empty(), "found {found:?}");
    }

    // ---- discover_extension_paths ----

    #[test]
    fn test_discover_paths_combines_local_global_configured_and_dedups() {
        // local dir: cwd/.pi-rs/extensions/local.ts
        let local = make_temp_dir(&[("local.ts", "")]);
        // global dir: agentDir/extensions/global.ts
        let global = make_temp_dir(&[]);
        fs::create_dir_all(global.path().join("extensions")).expect("mkdir");
        fs::write(global.path().join("extensions").join("global.ts"), "").expect("write");
        // configured: a standalone file
        let cfg = make_temp_dir(&[("configured.ts", "")]);

        let cwd = local.path().to_string_lossy().to_string();
        // create the .pi-rs/extensions layout under cwd
        fs::create_dir_all(local.path().join(CONFIG_DIR_NAME).join("extensions")).expect("mkdir");
        fs::write(
            local.path().join(CONFIG_DIR_NAME).join("extensions").join("local.ts"),
            "",
        )
        .expect("write");

        let configured = vec![cfg.path().join("configured.ts").to_string_lossy().to_string()];
        let result = discover_extension_paths(
            &configured,
            &cwd,
            &global.path().to_string_lossy(),
        );

        let expected_local = local
            .path()
            .join(CONFIG_DIR_NAME)
            .join("extensions")
            .join("local.ts");
        let expected_global = global.path().join("extensions").join("global.ts");
        let expected_cfg = cfg.path().join("configured.ts");

        assert_eq!(result.paths.len(), 3, "paths = {:?}", result.paths);
        assert!(result.paths.contains(&expected_local));
        assert!(result.paths.contains(&expected_global));
        assert!(result.paths.contains(&expected_cfg));
        assert_eq!(result.resolved_cwd, PathBuf::from(cwd));
    }

    #[test]
    fn test_discover_paths_dedups_same_file() {
        let dir = make_temp_dir(&[("x.ts", "")]);
        let cwd = dir.path().to_string_lossy().to_string();
        let abs = dir.path().join("x.ts").to_string_lossy().to_string();
        // pass the same path twice as configured
        let result = discover_extension_paths(&[abs.clone(), abs], &cwd, &cwd);
        assert_eq!(result.paths.len(), 1, "dedup failed: {:?}", result.paths);
    }

    #[cfg(unix)]
    #[test]
    fn test_discover_paths_dedups_symlink_and_real_path() {
        // `pi install` symlinks the extension into agent_dir/extensions/;
        // the same extension then appears via the symlink path AND the
        // configured real path. Dedup must canonicalize so they collapse to
        // one entry. Regression: lexical dedup kept both → double load.
        let dir = make_temp_dir(&[("pkg/index.ts", "")]);
        let ext_dir = make_temp_dir(&[]);
        let link = ext_dir.path().join("installed.pkg");
        std::os::unix::fs::symlink(dir.path().join("pkg"), &link).expect("symlink");

        let cwd = dir.path().to_string_lossy().to_string();
        let agent_dir = ext_dir.path().to_string_lossy().to_string();
        // configured path = the real path; discovery also finds the symlink
        let real = dir.path().join("pkg").join("index.ts").to_string_lossy().to_string();
        let result = discover_extension_paths(&[real], &cwd, &agent_dir);
        assert_eq!(result.paths.len(), 1, "dedup failed: {:?}", result.paths);
    }

    #[test]
    fn test_discover_paths_configured_directory_scanned() {
        let dir = make_temp_dir(&[("p/a.ts", ""), ("p/b.js", "")]);
        let cwd = dir.path().to_string_lossy().to_string();
        let subdir = dir.path().join("p").to_string_lossy().to_string();
        let result = discover_extension_paths(&[subdir], &cwd, &cwd);
        assert_eq!(result.paths.len(), 2, "paths = {:?}", result.paths);
    }

    #[test]
    fn test_discover_paths_nonexistent_configured_kept_as_is() {
        let dir = make_temp_dir(&[]);
        let cwd = dir.path().to_string_lossy().to_string();
        let bogus = dir.path().join("nope.ts").to_string_lossy().to_string();
        let result = discover_extension_paths(&[bogus], &cwd, &cwd);
        // non-existent file is still listed (loader will report the error)
        assert_eq!(result.paths, vec![dir.path().join("nope.ts")]);
    }

    // ---- ExtensionCache ----

    #[test]
    fn test_cache_insert_get_same_token() {
        let mut cache: ExtensionCache<&'static str> = ExtensionCache::new();
        let token = cache.use_cwd(PathBuf::from("/cwd"));
        let path = PathBuf::from("/cwd/ext.ts");
        cache.insert(&token, path.clone(), "factory");
        assert_eq!(cache.get(&token, &path), Some(&"factory"));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_cache_clear_invalidates_token() {
        let mut cache: ExtensionCache<i32> = ExtensionCache::new();
        let token = cache.use_cwd(PathBuf::from("/cwd"));
        cache.insert(&token, PathBuf::from("/cwd/a.ts"), 1);
        assert!(cache.is_current(&token));

        cache.clear();
        assert!(!cache.is_current(&token));
        assert_eq!(cache.get(&token, Path::new("/cwd/a.ts")), None);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_cache_cwd_change_clears() {
        let mut cache: ExtensionCache<i32> = ExtensionCache::new();
        let t1 = cache.use_cwd(PathBuf::from("/cwd1"));
        cache.insert(&t1, PathBuf::from("/cwd1/a.ts"), 1);
        assert_eq!(cache.len(), 1);

        // switching cwd clears the cache and makes t1 stale
        let t2 = cache.use_cwd(PathBuf::from("/cwd2"));
        assert!(!cache.is_current(&t1));
        assert!(cache.is_empty());
        assert!(cache.is_current(&t2));
        assert_eq!(cache.get(&t1, Path::new("/cwd1/a.ts")), None);
    }

    #[test]
    fn test_cache_same_cwd_does_not_clear() {
        let mut cache: ExtensionCache<i32> = ExtensionCache::new();
        let t1 = cache.use_cwd(PathBuf::from("/cwd"));
        cache.insert(&t1, PathBuf::from("/cwd/a.ts"), 1);
        let t2 = cache.use_cwd(PathBuf::from("/cwd"));
        assert!(cache.is_current(&t1));
        assert!(cache.is_current(&t2));
        assert_eq!(cache.len(), 1, "same cwd must not clear");
    }

    #[test]
    fn test_cache_insert_with_stale_token_is_noop() {
        let mut cache: ExtensionCache<i32> = ExtensionCache::new();
        let token = cache.use_cwd(PathBuf::from("/cwd"));
        cache.clear();
        // token is now stale
        cache.insert(&token, PathBuf::from("/cwd/a.ts"), 1);
        assert!(cache.is_empty(), "stale insert must be a no-op");
    }

    #[test]
    fn test_cache_generation_increments_on_clear() {
        let mut cache: ExtensionCache<()> = ExtensionCache::new();
        let t0 = cache.use_cwd(PathBuf::from("/cwd"));
        let gen0 = t0.generation;
        cache.clear();
        let t1 = cache.use_cwd(PathBuf::from("/cwd"));
        assert_eq!(t1.generation, gen0 + 1);
    }
}
