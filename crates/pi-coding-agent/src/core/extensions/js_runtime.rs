//! V8-backed runtime for loading TS/JS pi extensions at runtime.
//!
//! Feature-gated behind `js-runtime` (deno_core + deno_ast). This is the
//! factory-invocation half of TS `core/extensions/loader.ts`; the
//! V8-agnostic discovery/cache half lives in `loader.rs`.
//!
//! Status: foundational spike — proves the V8 build commitment works and that
//! a TS extension's default-export factory can be loaded and invoked with a
//! host-provided `pi` API object whose calls bridge back into Rust ops. The
//! full SDK shim (all register/action methods), two-phase lifecycle, EventBus
//! and provider bridge are later sub-chunks (see EXTENSION_LOADING_FEASIBILITY
//! §6). This module is NOT yet wired into the live run path and does NOT
//! reverse DEVIATIONS #5/#6.

#![cfg(feature = "js-runtime")]

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use deno_ast::MediaType;
use deno_ast::ParseParams;
use deno_ast::SourceMapOption;
use deno_core::anyhow;
use deno_core::error::ModuleLoaderError;
use deno_core::op2;
use deno_core::resolve_import;
use deno_core::resolve_path;
use deno_core::Extension;
use deno_core::JsRuntime;
use deno_core::ModuleLoadOptions;
use deno_core::ModuleLoadReferrer;
use deno_core::ModuleLoadResponse;
use deno_core::ModuleLoader;
use deno_core::ModuleSource;
use deno_core::ModuleSourceCode;
use deno_core::ModuleSpecifier;
use deno_core::ModuleType;
use deno_core::OpDecl;
use deno_core::OpState;
use deno_core::ResolutionKind;
use deno_core::RuntimeOptions;
use deno_error::JsErrorBox;
use serde::{Deserialize, Serialize};

// ============================================================================
// Load result — what a factory invocation leaves in Rust-side state
// ============================================================================

/// A tool that a loaded extension registered via `pi.registerTool`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LoadedToolRecord {
    pub name: String,
    pub description: String,
}

/// Everything a loaded extension factory registered into Rust state.
#[derive(Debug, Clone, Default)]
pub struct ExtensionLoadResult {
    pub tools: Vec<LoadedToolRecord>,
    pub commands: Vec<String>,
    pub logs: Vec<String>,
}

// ============================================================================
// Ops — the Rust side of the `pi` API surface (minimal subset for the spike)
// ============================================================================

#[op2(fast)]
fn op_register_tool(
    state: &mut OpState,
    #[string] name: String,
    #[string] description: String,
) -> Result<(), JsErrorBox> {
    let mut result = take_result(state);
    result.tools.push(LoadedToolRecord { name, description });
    state.put(result);
    Ok(())
}

#[op2(fast)]
fn op_register_command(
    state: &mut OpState,
    #[string] name: String,
) -> Result<(), JsErrorBox> {
    let mut result = take_result(state);
    result.commands.push(name);
    state.put(result);
    Ok(())
}

#[op2(fast)]
fn op_pi_log(state: &mut OpState, #[string] msg: String) -> Result<(), JsErrorBox> {
    let mut result = take_result(state);
    result.logs.push(msg);
    state.put(result);
    Ok(())
}

/// Take the accumulated load result out of `OpState`, leaving an empty one.
fn take_result(state: &mut OpState) -> ExtensionLoadResult {
    state.try_take::<ExtensionLoadResult>().unwrap_or_default()
}

/// The op declarations exposed to JS as `Deno.core.ops.op_*`.
const OPS: &[OpDecl] = &[
    op_register_tool(),
    op_register_command(),
    op_pi_log(),
];

fn pi_extension() -> Extension {
    Extension {
        name: "pi_ext",
        ops: Cow::Borrowed(OPS),
        ..Default::default()
    }
}

/// JS bootstrap that builds the `globalThis.__pi` API object wrapping the ops.
/// Mirrors the registration subset of TS `createExtensionAPI` (the full
/// surface is filled in by later sub-chunks).
const BOOTSTRAP_JS: &str = r#"
globalThis.__pi = {
  registerTool(t) {
    Deno.core.ops.op_register_tool(String(t.name), String(t.description ?? ""));
  },
  registerCommand(name) {
    Deno.core.ops.op_register_command(String(name));
  },
  log(msg) {
    Deno.core.ops.op_pi_log(String(msg));
  },
};
"#;

// ============================================================================
// TS module loader (swc transpile, no typecheck — like Deno --no-check)
// ============================================================================

type SourceMapStore = Rc<RefCell<HashMap<String, Vec<u8>>>>;

/// Module loader that reads `.ts`/`.js` files from disk and transpiles TS via
/// swc (deno_ast). Mirrors the `jiti.import` transpile step; no typechecking.
struct TypescriptModuleLoader {
    source_maps: SourceMapStore,
}

impl TypescriptModuleLoader {
    fn new() -> Self {
        Self {
            source_maps: Rc::new(RefCell::new(HashMap::new())),
        }
    }
}

impl ModuleLoader for TypescriptModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
        resolve_import(specifier, referrer).map_err(JsErrorBox::from_err)
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        let source_maps = self.source_maps.clone();
        ModuleLoadResponse::Sync(load(source_maps, module_specifier))
    }

    fn get_source_map(&self, specifier: &str) -> Option<Cow<'_, [u8]>> {
        self.source_maps
            .borrow()
            .get(specifier)
            .map(|v| Cow::Owned(v.clone()))
    }
}

/// Read + (if TS) transpile a module from disk. Free function so it can be
/// returned directly inside `ModuleLoadResponse::Sync`.
fn load(
    source_maps: SourceMapStore,
    module_specifier: &ModuleSpecifier,
) -> Result<ModuleSource, ModuleLoaderError> {
    let path = module_specifier
        .to_file_path()
        .map_err(|_| JsErrorBox::generic("Only file:// URLs are supported."))?;
    let media_type = MediaType::from_path(&path);
    let (module_type, should_transpile) = match media_type {
        MediaType::JavaScript | MediaType::Mjs | MediaType::Cjs => {
            (ModuleType::JavaScript, false)
        }
        MediaType::Jsx => (ModuleType::JavaScript, true),
        MediaType::TypeScript
        | MediaType::Mts
        | MediaType::Cts
        | MediaType::Dts
        | MediaType::Dmts
        | MediaType::Dcts
        | MediaType::Tsx => (ModuleType::JavaScript, true),
        MediaType::Json => (ModuleType::Json, false),
        _ => {
            return Err(JsErrorBox::generic(format!(
                "Unknown extension {:?}",
                path.extension()
            )));
        }
    };

    let code = std::fs::read_to_string(&path).map_err(JsErrorBox::from_err)?;
    let code = if should_transpile {
        let parsed = deno_ast::parse_module(ParseParams {
            specifier: module_specifier.clone(),
            text: code.into(),
            media_type,
            capture_tokens: false,
            scope_analysis: false,
            maybe_syntax: None,
        })
        .map_err(JsErrorBox::from_err)?;
        let res = parsed
            .transpile(
                &deno_ast::TranspileOptions {
                    imports_not_used_as_values: deno_ast::ImportsNotUsedAsValues::Remove,
                    ..Default::default()
                },
                &deno_ast::TranspileModuleOptions {
                    module_kind: None,
                },
                &deno_ast::EmitOptions {
                    source_map: SourceMapOption::Separate,
                    inline_sources: true,
                    ..Default::default()
                },
            )
            .map_err(JsErrorBox::from_err)?;
        let res = res.into_source();
        if let Some(sm) = res.source_map {
            source_maps
                .borrow_mut()
                .insert(module_specifier.to_string(), sm.into_bytes());
        }
        res.text
    } else {
        code
    };
    Ok(ModuleSource::new(
        module_type,
        ModuleSourceCode::String(code.into()),
        module_specifier,
        None,
    ))
}

// ============================================================================
// JsExtensionRuntime
// ============================================================================

/// A V8 runtime capable of loading and invoking a TS/JS extension's
/// default-export factory against a host-provided `pi` API object.
pub struct JsExtensionRuntime {
    runtime: JsRuntime,
}

impl JsExtensionRuntime {
    /// Create a new runtime with the `pi_ext` ops registered and the `__pi`
    /// bootstrap object installed.
    pub fn new() -> anyhow::Result<Self> {
        let mut runtime = JsRuntime::new(RuntimeOptions {
            extensions: vec![pi_extension()],
            module_loader: Some(Rc::new(TypescriptModuleLoader::new())),
            ..Default::default()
        });
        runtime
            .op_state()
            .borrow_mut()
            .put(ExtensionLoadResult::default());
        runtime.execute_script("<pi-bootstrap>", BOOTSTRAP_JS)?;
        Ok(Self { runtime })
    }

    /// Load and invoke the default-export factory of the TS/JS extension at
    /// `path` (resolved against `cwd`), passing `globalThis.__pi` as the API
    /// argument. Mirrors TS `jiti.import(path, { default: true })` followed by
    /// `factory(api)`.
    pub async fn load_extension(&mut self, path: &Path, cwd: &Path) -> anyhow::Result<()> {
        let ext_specifier = resolve_path(&path.to_string_lossy(), cwd)
            .map_err(|e| anyhow::anyhow!("resolve extension path: {e}"))?;
        // A shim module that imports the extension's default export and invokes
        // it with the host-provided `__pi` object. This mirrors jiti's
        // `{ default: true }` extraction + `factory(api)` call without manual
        // v8 namespace manipulation.
        let shim_code = format!(
            "import factory from \"{ext_url}\";\nawait factory(globalThis.__pi);\n",
            ext_url = ext_specifier.as_str()
        );
        // Use a file:// specifier (not ext:, which is reserved for deno_core
        // extensions) so the shim can freely import the extension's file:// URL.
        let shim_specifier = resolve_path("pi_loader/main.js", cwd)
            .map_err(|e| anyhow::anyhow!("invalid shim specifier: {e}"))?;

        let mod_id = self
            .runtime
            .load_main_es_module_from_code(&shim_specifier, shim_code)
            .await?;
        let result = self.runtime.mod_evaluate(mod_id);
        self.runtime.run_event_loop(Default::default()).await?;
        result.await?;
        Ok(())
    }

    /// Take the accumulated registration result out of the runtime's op state.
    pub fn take_result(&mut self) -> ExtensionLoadResult {
        self.runtime
            .op_state()
            .borrow_mut()
            .try_take::<ExtensionLoadResult>()
            .unwrap_or_default()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stdout)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::io::Write;

    /// Drive a deno_core runtime: it needs a single-threaded tokio runtime
    /// with all features enabled for the event loop to make progress.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime")
            .block_on(future)
    }

    fn write_extension(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        let mut f = fs::File::create(&path).expect("create");
        f.write_all(body.as_bytes()).expect("write");
        path
    }

    #[test]
    fn test_v8_links_and_sync_op_works() {
        // Minimal proof that V8 is linked and ops round-trip: execute a sync
        // script that calls op_pi_log.
        let mut runtime = JsRuntime::new(RuntimeOptions {
            extensions: vec![pi_extension()],
            ..Default::default()
        });
        runtime
            .op_state()
            .borrow_mut()
            .put(ExtensionLoadResult::default());
        runtime
            .execute_script("<sync-test>", "Deno.core.ops.op_pi_log('hello from v8');")
            .expect("execute_script");
        let result = runtime
            .op_state()
            .borrow_mut()
            .try_take::<ExtensionLoadResult>()
            .unwrap_or_default();
        assert_eq!(result.logs, vec!["hello from v8".to_string()]);
    }

    #[test]
    fn test_load_ts_extension_factory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = write_extension(
            dir.path(),
            "my-ext.ts",
            r#"
export default async function(pi) {
  pi.log("loading my-ext");
  pi.registerTool({ name: "search", description: "search the web" });
  pi.registerCommand("greet");
}
"#,
        );

        let mut js = JsExtensionRuntime::new().expect("runtime");
        block_on(js.load_extension(&ext, dir.path())).expect("load_extension");
        let result = js.take_result();
        assert_eq!(result.logs, vec!["loading my-ext".to_string()]);
        assert_eq!(
            result.tools,
            vec![LoadedToolRecord {
                name: "search".into(),
                description: "search the web".into(),
            }]
        );
        assert_eq!(result.commands, vec!["greet".to_string()]);
    }

    #[test]
    fn test_load_js_extension_factory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = write_extension(
            dir.path(),
            "plain.js",
            r#"
export default async function(pi) {
  pi.registerTool({ name: "t", description: "d" });
}
"#,
        );
        let mut js = JsExtensionRuntime::new().expect("runtime");
        block_on(js.load_extension(&ext, dir.path())).expect("load_extension");
        let result = js.take_result();
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "t");
    }

    #[test]
    fn test_extension_with_imports() {
        // An extension that imports a local helper module — proves the module
        // loader resolves relative imports.
        let dir = tempfile::tempdir().expect("tempdir");
        write_extension(
            dir.path(),
            "util.ts",
            "export function upper(s) { return s.toUpperCase(); }",
        );
        let ext = write_extension(
            dir.path(),
            "main.ts",
            r#"
import { upper } from "./util.ts";
export default async function(pi) {
  pi.log(upper("loaded"));
}
"#,
        );
        let mut js = JsExtensionRuntime::new().expect("runtime");
        block_on(js.load_extension(&ext, dir.path())).expect("load_extension");
        let result = js.take_result();
        assert_eq!(result.logs, vec!["LOADED".to_string()]);
    }
}
