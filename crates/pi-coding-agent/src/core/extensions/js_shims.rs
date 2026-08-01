//! Embedded JS shims for bare-specifier imports used by pi extensions.
//!
//! When loading TS/JS extensions via V8, extensions import from packages like
//! `typebox`, `@earendil-works/pi-ai`, `@earendil-works/pi-tui`, and
//! `@earendil-works/pi-coding-agent`. Since there is no `node_modules` in the
//! V8 runtime, we intercept these bare specifiers in the module loader and
//! return minimal embedded JS that provides the runtime API surface extensions
//! actually use.
//!
//! ## typebox
//!
//! Only the `Type.*` factory methods that produce JSON Schema objects are
//! shimmed: `String`, `Number`, `Boolean`, `Object`, `Array`, `Optional`,
//! `Literal`, `Union`, `Unsafe`. The `~kind` and `~optional` markers are set
//! as **non-enumerable** properties (matching typebox's default
//! `enumerableKind: false`), so `JSON.stringify(schema)` produces clean JSON
//! Schema without internal markers — exactly like the real typebox.
//!
//! ## @earendil-works/pi-ai
//!
//! Only `StringEnum` is exported (the one helper extensions use). It delegates
//! to `Type.Unsafe` to produce `{type:"string", enum:[...], ...options}`.
//!
//! ## @earendil-works/pi-tui
//!
//! TUI is not ported (confirmed deviation, see DEVIATIONS.md). All exports are
//! stubs that throw "TUI unavailable in this mode" when called. Extensions
//! that only *import* TUI components but never call them at runtime (e.g.
//! conditional on `ctx.mode === "tui"`) will load successfully.
//!
//! ## @earendil-works/pi-coding-agent
//!
//! Exports the simple constants (`VERSION`, `CONFIG_DIR_NAME`, `APP_NAME`) and
//! utility functions (`getAgentDir`, `defineTool`, `parseFrontmatter`,
//! `withFileMutationQueue`) that extensions commonly use. Complex exports
//! (`createBashTool`, `serializeConversation`, TUI component classes) are stubs
//! that throw.

#![cfg(feature = "js-runtime")]

/// The URL scheme used for synthetic shim modules.
pub const SHIM_SCHEME: &str = "pi-shim";

/// All bare specifiers we intercept, mapped to their shim module path.
pub const SHIM_SPECIFIERS: &[(&str, &str)] = &[
    ("typebox", "typebox"),
    ("@earendil-works/pi-ai", "pi-ai"),
    ("@earendil-works/pi-tui", "pi-tui"),
    ("@earendil-works/pi-coding-agent", "pi-coding-agent"),
    // Node.js built-in modules (both `node:` and bare specifier forms).
    ("node:fs", "node-fs"), ("fs", "node-fs"),
    ("node:path", "node-path"), ("path", "node-path"),
    ("node:os", "node-os"), ("os", "node-os"),
    ("node:child_process", "node-cp"), ("child_process", "node-cp"),
    ("node:util", "node-util"), ("util", "node-util"),
    ("node:url", "node-url"), ("url", "node-url"),
    ("node:module", "node-module"), ("module", "node-module"),
    ("node:process", "node-process"), ("process", "node-process"),
    ("node:readline", "node-readline"), ("readline", "node-readline"),
    ("node:fs/promises", "node-fs-promises"), ("fs/promises", "node-fs-promises"),
];

/// Look up a bare specifier and return the shim module path if recognized.
/// Handles both exact matches and subpath imports (e.g.
/// `@earendil-works/pi-ai/compat`, `node:fs/promises`, `fs/promises`).
pub fn lookup_shim(specifier: &str) -> Option<&'static str> {
    // Exact match first.
    if let Some((_, path)) = SHIM_SPECIFIERS.iter().find(|(spec, _)| *spec == specifier) {
        return Some(*path);
    }
    // Subpath imports: match the package prefix and map to a subpath shim.
    // e.g. "@earendil-works/pi-ai/compat" -> "pi-ai-compat"
    //      "node:fs/promises" -> "node-fs"
    //      "fs/promises" -> "node-fs"
    for (prefix, base_path) in SHIM_SPECIFIERS {
        if specifier.starts_with(prefix) && specifier.as_bytes().get(prefix.len()) == Some(&b'/') {
            // Subpath under a known package. For most, we return the base
            // shim (the subpath API is a subset). For pi-ai/compat, we have
            // a dedicated shim.
            let subpath = &specifier[prefix.len() + 1..];
            if *base_path == "pi-ai" && subpath == "compat" {
                return Some("pi-ai-compat");
            }
            // For fs/promises, return the same fs stub.
            if *base_path == "node-fs" && subpath == "promises" {
                return Some("node-fs");
            }
            // Default: return the base shim.
            return Some(*base_path);
        }
    }
    None
}

/// Build the full `pi-shim://` URL for a given shim module path.
pub fn shim_url(shim_path: &str) -> String {
    format!("{SHIM_SCHEME}://{shim_path}")
}

/// Check if a module specifier URL is a shim module.
pub fn is_shim_specifier(url: &str) -> bool {
    url.starts_with(&format!("{SHIM_SCHEME}://"))
}

/// Get the shim JS source for a given shim module path.
pub fn shim_source(shim_path: &str) -> Option<&'static str> {
    match shim_path {
        "typebox" => Some(TYPEBOX_SHIM),
        "pi-ai" => Some(PI_AI_SHIM),
        "pi-tui" => Some(PI_TUI_SHIM),
        "pi-coding-agent" => Some(PI_CODING_AGENT_SHIM),
        "node-fs" => Some(NODE_FS_SHIM),
        "node-path" => Some(NODE_PATH_SHIM),
        "node-os" => Some(NODE_OS_SHIM),
        "node-cp" => Some(NODE_CP_SHIM),
        "node-util" => Some(NODE_UTIL_SHIM),
        "node-url" => Some(NODE_URL_SHIM),
        "node-module" => Some(NODE_MODULE_SHIM),
        "node-process" => Some(NODE_PROCESS_SHIM),
        "node-readline" => Some(NODE_READLINE_SHIM),
        "node-fs-promises" => Some(NODE_FS_PROMISES_SHIM),
        "pi-ai-compat" => Some(PI_AI_COMPAT_SHIM),
        _ => None,
    }
}

// ============================================================================
// typebox shim
// ============================================================================

/// Minimal typebox shim providing the `Type` factory methods used by pi
/// extensions. Produces JSON Schema objects with non-enumerable `~kind` /
/// `~optional` markers (matching typebox's default `enumerableKind: false`).
const TYPEBOX_SHIM: &str = r#"
// Minimal typebox shim — provides Type.* factory methods that produce JSON
// Schema objects. Internal markers (~kind, ~optional) are non-enumerable so
// JSON.stringify produces clean JSON Schema (matching typebox defaults).

function defineHidden(obj, key, value) {
  Object.defineProperty(obj, key, {
    configurable: true,
    writable: true,
    enumerable: false,
    value,
  });
  return obj;
}

function create(kind, schema, options) {
  const result = { ...schema, ...(options || {}) };
  defineHidden(result, "~kind", kind);
  return result;
}

function cloneWithMarker(obj, key, value) {
  // Shallow clone preserving property descriptors (for non-enumerable ~kind).
  const result = {};
  const descs = Object.getOwnPropertyDescriptors(obj);
  for (const k of Object.keys(descs)) {
    Object.defineProperty(result, k, descs[k]);
  }
  defineHidden(result, key, value);
  return result;
}

function isOptional(schema) {
  return Object.prototype.hasOwnProperty.call(schema, "~optional");
}

const Type = {
  String(options) {
    return create("String", { type: "string" }, options);
  },
  Number(options) {
    return create("Number", { type: "number" }, options);
  },
  Boolean(options) {
    return create("Boolean", { type: "boolean" }, options);
  },
  Object(properties, options = {}) {
    const required = Object.keys(properties).filter(k => !isOptional(properties[k]));
    const schema = { type: "object", properties };
    if (required.length > 0) schema.required = required;
    return create("Object", schema, options);
  },
  Array(items, options) {
    return create("Array", { type: "array", items }, options);
  },
  Optional(type) {
    return cloneWithMarker(type, "~optional", true);
  },
  Literal(value, options) {
    const typeName = typeof value === "number" ? "number"
      : typeof value === "boolean" ? "boolean"
      : typeof value === "bigint" ? "bigint"
      : "string";
    return create("Literal", { type: typeName, const: value }, options);
  },
  Union(anyOf, options = {}) {
    return create("Union", { anyOf }, options);
  },
  Unsafe(schema) {
    return cloneWithMarker(schema, "~unsafe", null);
  },
};

export { Type };
export default Type;
"#;

// ============================================================================
// @earendil-works/pi-ai shim
// ============================================================================

/// Shim for `@earendil-works/pi-ai` — provides `StringEnum`, the only runtime
/// export used by extensions. Delegates to the typebox `Type.Unsafe` factory.
const PI_AI_SHIM: &str = r#"
import { Type } from "typebox";

/**
 * Creates a string enum schema compatible with Google's API and other providers
 * that don't support anyOf/const patterns.
 *
 * Mirrors @earendil-works/pi-ai src/utils/typebox-helpers.ts.
 */
export function StringEnum(values, options) {
  const schema = {
    type: "string",
    enum: values,
    ...(options?.description && { description: options.description }),
    ...(options?.default !== undefined && { default: options.default }),
  };
  return Type.Unsafe(schema);
}

// Re-export Type from typebox (some extensions import it from pi-ai).
export { Type } from "typebox";

export default { StringEnum, Type };
"#;

// ============================================================================
// @earendil-works/pi-tui shim (stub — TUI not ported)
// ============================================================================

/// Stub for `@earendil-works/pi-tui`. All exports are functions/classes that
/// throw "TUI unavailable" when called. Extensions that import TUI components
/// but never invoke them at runtime (e.g. guarded by `ctx.mode === "tui"`)
/// will load successfully.
const PI_TUI_SHIM: &str = r#"
function tuiUnavailable(name) {
  throw new Error(
    `TUI component '${name}' is unavailable: pi-tui is not ported in this mode. ` +
    `See DEVIATIONS.md — TUI rendering layer is a confirmed deviation.`
  );
}

// Component classes — throw on construction.
export class Box { constructor() { tuiUnavailable("Box"); } }
export class Container { constructor() { tuiUnavailable("Container"); } }
export class Input { constructor() { tuiUnavailable("Input"); } }
export class Text { constructor() { tuiUnavailable("Text"); } }
export class Markdown { constructor() { tuiUnavailable("Markdown"); } }
export class DynamicBorder { constructor() { tuiUnavailable("DynamicBorder"); } }
export class BorderedLoader { constructor() { tuiUnavailable("BorderedLoader"); } }
export class CustomEditor { constructor() { tuiUnavailable("CustomEditor"); } }

// More component classes.
export class Editor { constructor() { tuiUnavailable("Editor"); } }
export class SettingsList { constructor() { tuiUnavailable("SettingsList"); } }
export class SelectList { constructor() { tuiUnavailable("SelectList"); } }

// Utility functions — throw on call.
export function matchesKey() { tuiUnavailable("matchesKey"); }
export function truncateToWidth() { tuiUnavailable("truncateToWidth"); }
export function getMarkdownTheme() { tuiUnavailable("getMarkdownTheme"); }
export function getSettingsListTheme() { tuiUnavailable("getSettingsListTheme"); }
export function visibleWidth() { tuiUnavailable("visibleWidth"); }
export function fuzzyFilter() { tuiUnavailable("fuzzyFilter"); }
export function wrapTextWithAnsi() { tuiUnavailable("wrapTextWithAnsi"); }
export function isKeyRelease() { tuiUnavailable("isKeyRelease"); }

// Constants — Key is a Proxy that returns a function for any property
// access (extensions use Key.ctrlC, Key.shiftTab, etc. as matchers).
export const Key = new Proxy({}, {
  get(target, prop) {
    if (prop in target) return target[prop];
    if (typeof prop === "string") {
      return function() { tuiUnavailable("Key." + prop); };
    }
    return undefined;
  },
});
export const CURSOR_MARKER = "";

// Generic Proxy fallback for any other named export.
const handler = {
  get(target, prop) {
    if (prop in target) return target[prop];
    if (typeof prop === "string") {
      return function() { tuiUnavailable(prop); };
    }
    return undefined;
  },
};

const allExports = {
  Box, Container, Input, Text, Markdown, DynamicBorder, BorderedLoader,
  CustomEditor, Editor, SettingsList,
  matchesKey, truncateToWidth, getMarkdownTheme, getSettingsListTheme,
  visibleWidth, fuzzyFilter, wrapTextWithAnsi, isKeyRelease,
  Key, CURSOR_MARKER,
};

export default new Proxy(allExports, handler);
"#;

// ============================================================================
// @earendil-works/pi-coding-agent shim
// ============================================================================

/// Shim for `@earendil-works/pi-coding-agent`. Provides the simple constants
/// and utility functions extensions commonly use at runtime. Complex exports
/// (createBashTool, serializeConversation, etc.) are stubs that throw.
const PI_CODING_AGENT_SHIM: &str = r#"
import { Type } from "typebox";

// ---- Constants (from config.ts) ----
export const VERSION = "1.79.41";
export const CONFIG_DIR_NAME = ".pi";
export const APP_NAME = "pi";
export const APP_TITLE = "π";
export const PACKAGE_NAME = "@earendil-works/pi-coding-agent";
export const CURRENT_SESSION_VERSION = 3;

// ---- Utility functions ----

export function getAgentDir() {
  const env = globalThis.__piAgentDir;
  if (env) return env;
  // Best-effort: ~/.pi/agent (mirrors TS getAgentDir)
  const home = globalThis.__piHomeDir || ".";
  return home + "/" + CONFIG_DIR_NAME + "/agent";
}

export function getModelsPath() {
  return getAgentDir() + "/models.json";
}

export function getCustomThemesDir() {
  return getAgentDir() + "/themes";
}

/** defineTool is a type-level identity function — at runtime it just returns
 *  the tool object unchanged. */
export function defineTool(tool) {
  return tool;
}

/** Parse YAML frontmatter from a markdown string. Mirrors TS parseFrontmatter. */
export function parseFrontmatter(content) {
  const match = content.match(/^---\n([\s\S]*?)\n---\n([\s\S]*)$/);
  if (!match) return { frontmatter: {}, body: content };
  // Simple YAML parsing for key: value pairs (no nested structures).
  const frontmatter = {};
  for (const line of match[1].split("\n")) {
    const idx = line.indexOf(":");
    if (idx === -1) continue;
    const key = line.slice(0, idx).trim();
    const value = line.slice(idx + 1).trim();
    frontmatter[key] = value;
  }
  return { frontmatter, body: match[2] };
}

/** Serialize with file mutation queue — stub that just calls the fn. */
export function withFileMutationQueue(fn) {
  return fn();
}

/** Convert conversation to LLM messages — stub. */
export function convertToLlm() {
  throw new Error("convertToLlm is not available in the extension runtime.");
}

/** Serialize conversation — stub. */
export function serializeConversation() {
  throw new Error("serializeConversation is not available in the extension runtime.");
}

// ---- Tool factory stubs ----
export function createBashTool() {
  throw new Error("createBashTool is not available in the extension runtime.");
}
export function createReadTool() {
  throw new Error("createReadTool is not available in the extension runtime.");
}
export function createWriteTool() {
  throw new Error("createWriteTool is not available in the extension runtime.");
}
export function createEditTool() {
  throw new Error("createEditTool is not available in the extension runtime.");
}
export function createFindTool() {
  throw new Error("createFindTool is not available in the extension runtime.");
}
export function createGrepTool() {
  throw new Error("createGrepTool is not available in the extension runtime.");
}
export function createLsTool() {
  throw new Error("createLsTool is not available in the extension runtime.");
}

// ---- Utility functions ----
export function formatSize(bytes) {
  if (bytes < 1024) return bytes + " B";
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + " KB";
  return (bytes / (1024 * 1024)).toFixed(1) + " MB";
}

// ---- Truncation utilities (from core/tools/truncate.ts) ----
export function truncateHead(content, options = {}) {
  const maxBytes = options.maxBytes || 51200;
  const maxLines = options.maxLines || 2000;
  const lines = content.split("\n");
  if (lines.length <= maxLines && content.length <= maxBytes) {
    return { content, truncated: false };
  }
  const truncatedLines = lines.slice(0, maxLines);
  let result = truncatedLines.join("\n");
  if (result.length > maxBytes) {
    result = result.slice(0, maxBytes);
  }
  return { content: result, truncated: true };
}

export function truncateTail(content, options = {}) {
  const maxBytes = options.maxBytes || 51200;
  const maxLines = options.maxLines || 2000;
  const lines = content.split("\n");
  if (lines.length <= maxLines && content.length <= maxBytes) {
    return { content, truncated: false };
  }
  const truncatedLines = lines.slice(-maxLines);
  let result = truncatedLines.join("\n");
  if (result.length > maxBytes) {
    result = result.slice(-maxBytes);
  }
  return { content: result, truncated: true };
}

// ---- Re-exports from pi-tui (stubs) ----
export function getMarkdownTheme() {
  throw new Error("getMarkdownTheme: TUI not available.");
}
export function getSettingsListTheme() {
  throw new Error("getSettingsListTheme: TUI not available.");
}
export const DEFAULT_MAX_BYTES = 51200;  // 50KB
export const DEFAULT_MAX_LINES = 2000;

// ---- TUI component re-exports (stubs) ----
export class BorderedLoader { constructor() { throw new Error("BorderedLoader: TUI not available."); } }
export class DynamicBorder { constructor() { throw new Error("DynamicBorder: TUI not available."); } }
export class CustomEditor { constructor() { throw new Error("CustomEditor: TUI not available."); } }

export default {
  VERSION, CONFIG_DIR_NAME, APP_NAME, APP_TITLE, PACKAGE_NAME,
  CURRENT_SESSION_VERSION,
  getAgentDir, getModelsPath, getCustomThemesDir,
  defineTool, parseFrontmatter, withFileMutationQueue,
  convertToLlm, serializeConversation,
  createBashTool, createReadTool, createWriteTool, createEditTool,
  createFindTool, createGrepTool, createLsTool,
  getMarkdownTheme, getSettingsListTheme, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES,
  formatSize, truncateHead, truncateTail,
  BorderedLoader, DynamicBorder, CustomEditor,
};
"#;

// ============================================================================
// Node.js built-in module shims
// ============================================================================

/// Minimal `node:path` implementation — provides the string-manipulation
/// functions extensions commonly use (`join`, `dirname`, `resolve`, `basename`,
/// `extname`). These are pure string operations that don't touch the filesystem.
const NODE_PATH_SHIM: &str = r#"
const SEP = "/";

function normalize(p) {
  // Simplified POSIX normalize: collapse . and .. segments.
  const parts = p.split(SEP);
  const result = [];
  for (const part of parts) {
    if (part === "" || part === ".") continue;
    if (part === "..") { result.pop(); continue; }
    result.push(part);
  }
  let ret = result.join(SEP);
  if (p.startsWith("/")) ret = "/" + ret;
  if (p.endsWith("/") && !ret.endsWith("/")) ret += "/";
  return ret || ".";
}

export function join(...args) {
  return normalize(args.filter(Boolean).join(SEP));
}

export function dirname(p) {
  const idx = p.lastIndexOf(SEP);
  if (idx === -1) return ".";
  if (idx === 0) return "/";
  return p.slice(0, idx);
}

export function basename(p, ext) {
  const name = p.split(SEP).filter(Boolean).pop() || "";
  if (ext && name.endsWith(ext)) return name.slice(0, -ext.length);
  return name;
}

export function extname(p) {
  const base = basename(p);
  const idx = base.lastIndexOf(".");
  if (idx === 0 || idx === -1) return "";
  return base.slice(idx);
}

export function resolve(...args) {
  // Simplified: just join with leading /.
  let p = args.filter(Boolean).join(SEP);
  if (!p.startsWith("/")) p = "/" + p;
  return normalize(p);
}

export function relative(from, to) {
  // Simplified: just return the absolute difference.
  return normalize(to);
}

export function isAbsolute(p) {
  return p.startsWith("/");
}

export const sep = SEP;
export const delimiter = ":";

export default { join, dirname, basename, extname, resolve, relative, isAbsolute, sep, delimiter };
"#;

/// Minimal `node:os` — provides `homedir`, `tmpdir`, `platform` using
/// globals set by the runtime (or best-effort defaults).
const NODE_OS_SHIM: &str = r#"
export function homedir() {
  return globalThis.__piHomeDir || "/tmp";
}
export function tmpdir() {
  return globalThis.__piTmpDir || "/tmp";
}
export function platform() {
  return globalThis.__piPlatform || "linux";
}
export function arch() {
  return globalThis.__piArch || "arm64";
}
export function hostname() {
  return globalThis.__piHostname || "localhost";
}
export const EOL = "\n";

export default { homedir, tmpdir, platform, arch, hostname, EOL };
"#;

/// `node:fs` stub — all functions throw. Extensions that use `fs` only in
/// event handlers (not during factory load) will load successfully.
const NODE_FS_SHIM: &str = r#"
export function readFileSync(path, options) {
  return Deno.core.ops.op_fs_read_file_sync(String(path));
}

export function writeFileSync(path, data, options) {
  Deno.core.ops.op_fs_write_file_sync(String(path), String(data));
}

export function appendFileSync(path, data, options) {
  Deno.core.ops.op_fs_append_file_sync(String(path), String(data));
}

export function existsSync(path) {
  return Deno.core.ops.op_fs_exists_sync(String(path));
}

export function mkdirSync(path, options) {
  const recursive = options && (options.recursive === true);
  Deno.core.ops.op_fs_mkdir_sync(String(path), recursive);
}

export function readdirSync(path, options) {
  const result = Deno.core.ops.op_fs_readdir_sync(String(path));
  return JSON.parse(result);
}

export function statSync(path, options) {
  const result = Deno.core.ops.op_fs_stat_sync(String(path));
  const s = JSON.parse(result);
  return {
    size: s.size,
    isFile: () => s.isFile,
    isDirectory: () => s.isDirectory,
    isSymbolicLink: () => s.isSymlink,
    mode: s.mode,
    mtimeMs: s.mtimeMs,
    mtime: s.mtimeMs ? new Date(s.mtimeMs) : undefined,
  };
}

export function unlinkSync(path) {
  Deno.core.ops.op_fs_unlink_sync(String(path));
}

export function rmSync(path, options) {
  const recursive = options && (options.recursive === true);
  Deno.core.ops.op_fs_rm_sync(String(path), recursive);
}

export function copyFileSync(src, dest, flags) {
  Deno.core.ops.op_fs_copy_file_sync(String(src), String(dest));
}

export function renameSync(oldPath, newPath) {
  Deno.core.ops.op_fs_rename_sync(String(oldPath), String(newPath));
}

export function accessSync(path, mode) {
  Deno.core.ops.op_fs_access_sync(String(path));
}

export function mkdtempSync(prefix, options) {
  return Deno.core.ops.op_fs_mkdtemp_sync(String(prefix));
}

export function readFile(path, options) {
  return Promise.resolve(Deno.core.ops.op_fs_read_file_sync(String(path)));
}

export function writeFile(path, data, options) {
  Deno.core.ops.op_fs_write_file_sync(String(path), String(data));
  return Promise.resolve();
}

export function appendFile(path, data, options) {
  Deno.core.ops.op_fs_append_file_sync(String(path), String(data));
  return Promise.resolve();
}

export function access(path, mode) {
  Deno.core.ops.op_fs_access_sync(String(path));
  return Promise.resolve();
}

export function mkdir(path, options) {
  const recursive = options && (options.recursive === true);
  Deno.core.ops.op_fs_mkdir_sync(String(path), recursive);
  return Promise.resolve();
}

export function readdir(path, options) {
  const result = Deno.core.ops.op_fs_readdir_sync(String(path));
  return Promise.resolve(JSON.parse(result));
}

export function stat(path, options) {
  const result = Deno.core.ops.op_fs_stat_sync(String(path));
  const s = JSON.parse(result);
  return Promise.resolve({
    size: s.size,
    isFile: () => s.isFile,
    isDirectory: () => s.isDirectory,
    isSymbolicLink: () => s.isSymlink,
    mode: s.mode,
    mtimeMs: s.mtimeMs,
    mtime: s.mtimeMs ? new Date(s.mtimeMs) : undefined,
  });
}

export function unlink(path) {
  Deno.core.ops.op_fs_unlink_sync(String(path));
  return Promise.resolve();
}

export function rm(path, options) {
  const recursive = options && (options.recursive === true);
  Deno.core.ops.op_fs_rm_sync(String(path), recursive);
  return Promise.resolve();
}

export function copyFile(src, dest, flags) {
  Deno.core.ops.op_fs_copy_file_sync(String(src), String(dest));
  return Promise.resolve();
}

export function rename(oldPath, newPath) {
  Deno.core.ops.op_fs_rename_sync(String(oldPath), String(newPath));
  return Promise.resolve();
}

export function mkdtemp(prefix, options) {
  return Promise.resolve(Deno.core.ops.op_fs_mkdtemp_sync(String(prefix)));
}

export function watch() {
  throw new Error("node:fs.watch is not available in the extension runtime.");
}

export function createReadStream() {
  throw new Error("node:fs.createReadStream is not available in the extension runtime.");
}

export function createWriteStream() {
  throw new Error("node:fs.createWriteStream is not available in the extension runtime.");
}

export const promises = {
  readFile,
  writeFile,
  appendFile,
  access,
  mkdir,
  readdir,
  stat,
  unlink,
  rm,
  copyFile,
  rename,
  mkdtemp,
};

export const constants = {};
export default { readFileSync, writeFileSync, appendFileSync, existsSync, mkdirSync, readdirSync, statSync, unlinkSync, rmSync, copyFileSync, renameSync, accessSync, mkdtempSync, readFile, writeFile, appendFile, access, mkdir, readdir, stat, unlink, rm, copyFile, rename, mkdtemp, promises, constants };
"#;

/// `node:child_process` — `execSync` backed by Rust op; others throw.
const NODE_CP_SHIM: &str = r#"
export function execSync(command, options) {
  return Deno.core.ops.op_cp_exec_sync(String(command));
}

export function spawn() {
  throw new Error("node:child_process.spawn is not available in the extension runtime.");
}

export function exec() {
  throw new Error("node:child_process.exec is not available in the extension runtime.");
}

export function spawnSync() {
  throw new Error("node:child_process.spawnSync is not available in the extension runtime.");
}

export function fork() {
  throw new Error("node:child_process.fork is not available in the extension runtime.");
}

export default { execSync, spawn, exec, spawnSync, fork };
"#;

/// `node:util` — `promisify` wraps a callback-style function; `inspect` is
/// a simple stringifier.
const NODE_UTIL_SHIM: &str = r#"
export function promisify(fn) {
  return function(...args) {
    return new Promise((resolve, reject) => {
      fn(...args, (err, ...results) => {
        if (err) reject(err);
        else resolve(results.length > 1 ? results : results[0]);
      });
    });
  };
}
export function inspect(obj) {
  return String(obj);
}
export function format(...args) {
  return args.join(" ");
}
export function deprecate(fn) { return fn; }
export function callbackify(fn) {
  return function(...args) {
    const cb = args.pop();
    fn(...args).then(r => cb(null, r), e => cb(e));
  };
}

export default { promisify, inspect, format, deprecate, callbackify };
"#;

/// `node:url` — `fileURLToPath` extracts the path from a file:// URL.
const NODE_URL_SHIM: &str = r#"
export function fileURLToPath(url) {
  if (typeof url === "string") {
    return url.replace(/^file:\/\//, "");
  }
  if (url && url.href) {
    return url.href.replace(/^file:\/\//, "");
  }
  return String(url);
}

export function pathToFileURL(path) {
  return { href: "file://" + path, pathname: path };
}

export default { fileURLToPath, pathToFileURL };
"#;

/// `node:module` — `createRequire` loads JSON files via the fs op.
const NODE_MODULE_SHIM: &str = r#"
export function createRequire(path) {
  const baseDir = (typeof path === "string" ? path : "/").split("/").slice(0, -1).join("/") || "/";
  return function(modulePath) {
    // Resolve relative paths against the base directory
    const resolved = modulePath.startsWith(".") 
      ? (baseDir + "/" + modulePath).replace(/\/\.\//g, "/").replace(/\/\/+/g, "/")
      : modulePath;
    // Try to read as JSON
    try {
      const content = Deno.core.ops.op_fs_read_file_sync(resolved);
      return JSON.parse(content);
    } catch (e) {
      // Try with .json extension
      try {
        const content = Deno.core.ops.op_fs_read_file_sync(resolved + ".json");
        return JSON.parse(content);
      } catch (e2) {
        throw new Error("Cannot find module '" + modulePath + "'");
      }
    }
  };
}

export default { createRequire };
"#;

/// `node:process` — minimal environment access.
const NODE_PROCESS_SHIM: &str = r#"
const env = globalThis.__piEnv || {};
export const platform = globalThis.__piPlatform || "linux";
export const arch = globalThis.__piArch || "arm64";
export const cwd = () => globalThis.__piCwd || ".";
export const env_env = env;
export { env_env as env };

export default { platform, arch, cwd, env };
"#;

/// `node:readline` — basic `createInterface` that reads from a string input.
const NODE_READLINE_SHIM: &str = r#"
export function createInterface(options) {
  const input = options && options.input;
  const lines = [];
  if (input && typeof input === "object" && input._readableState) {
    // Stream-like input: not supported, return a no-op interface
    return {
      on() { return this; },
      once() { return this; },
      close() {},
      write() {},
      prompt() {},
      question() { return Promise.resolve(""); },
    };
  }
  return {
    on() { return this; },
    once() { return this; },
    close() {},
    write() {},
    prompt() {},
    question() { return Promise.resolve(""); },
  };
}

export default { createInterface };
"#;

/// `@earendil-works/pi-ai/compat` — stub for the legacy compat API.
/// All functions throw; extensions that use `complete`/`getModel` etc. at
/// runtime will fail, but the import resolves so factory load can proceed
/// if the functions aren't called during load.
const PI_AI_COMPAT_SHIM: &str = r#"
function unavailable(name) {
  throw new Error(`@earendil-works/pi-ai/compat.${name}() is not available in the extension runtime. ` +
    `Use pi.registerProvider for custom providers instead.`);
}

export function complete() { unavailable("complete"); }
export function stream() { unavailable("stream"); }
export function streamSimple() { unavailable("streamSimple"); }
export function getModel() { unavailable("getModel"); }
export function getModels() { unavailable("getModels"); }
export function getProviders() { unavailable("getProviders"); }
export function registerApiProvider() { unavailable("registerApiProvider"); }

export default { complete, stream, streamSimple, getModel, getModels, getProviders, registerApiProvider };
"#;

/// `node:fs/promises` — backed by Rust ops for real file system access.
const NODE_FS_PROMISES_SHIM: &str = r#"
export function readFile(path, options) {
  return Promise.resolve(Deno.core.ops.op_fs_read_file_sync(String(path)));
}

export function writeFile(path, data, options) {
  Deno.core.ops.op_fs_write_file_sync(String(path), String(data));
  return Promise.resolve();
}

export function appendFile(path, data, options) {
  Deno.core.ops.op_fs_append_file_sync(String(path), String(data));
  return Promise.resolve();
}

export function access(path, mode) {
  Deno.core.ops.op_fs_access_sync(String(path));
  return Promise.resolve();
}

export function mkdir(path, options) {
  const recursive = options && (options.recursive === true);
  Deno.core.ops.op_fs_mkdir_sync(String(path), recursive);
  return Promise.resolve();
}

export function readdir(path, options) {
  const result = Deno.core.ops.op_fs_readdir_sync(String(path));
  return Promise.resolve(JSON.parse(result));
}

export function stat(path, options) {
  const result = Deno.core.ops.op_fs_stat_sync(String(path));
  const s = JSON.parse(result);
  return Promise.resolve({
    size: s.size,
    isFile: () => s.isFile,
    isDirectory: () => s.isDirectory,
    isSymbolicLink: () => s.isSymlink,
    mode: s.mode,
    mtimeMs: s.mtimeMs,
    mtime: s.mtimeMs ? new Date(s.mtimeMs) : undefined,
  });
}

export function unlink(path) {
  Deno.core.ops.op_fs_unlink_sync(String(path));
  return Promise.resolve();
}

export function rm(path, options) {
  const recursive = options && (options.recursive === true);
  Deno.core.ops.op_fs_rm_sync(String(path), recursive);
  return Promise.resolve();
}

export function copyFile(src, dest, flags) {
  Deno.core.ops.op_fs_copy_file_sync(String(src), String(dest));
  return Promise.resolve();
}

export function rename(oldPath, newPath) {
  Deno.core.ops.op_fs_rename_sync(String(oldPath), String(newPath));
  return Promise.resolve();
}

export function mkdtemp(prefix, options) {
  return Promise.resolve(Deno.core.ops.op_fs_mkdtemp_sync(String(prefix)));
}

export default { readFile, writeFile, appendFile, access, mkdir, readdir, stat, unlink, rm, copyFile, rename, mkdtemp };
"#;
