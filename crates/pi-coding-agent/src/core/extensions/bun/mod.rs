//! Bun 子进程扩展运行时（方案 A）。
//!
//! 把 Bun 二进制嵌入发布产物（`assets/runtime/`，xtask `fetch-bun` 下载），
//! 运行时解压到缓存目录，spawn 子进程加载 TS/JS 扩展。扩展跑在**真实 Bun**
//! 里：真实 node_modules 解析（第三方依赖全通）、真实 node:fs/crypto/path
//! 等 Node API——不再需要手写 Node shim。
//!
//! 与宿主通信走 stdio JSON-RPC（newline-delimited）：
//! - 宿主 → Bun：`{"id":n,"method":"execute_tool","params":{...}}` 等请求
//! - Bun → 宿主：`{"method":"register_tool","params":{...}}` 等通知
//!
//! SDK 包（`@earendil-works/pi-*`、typebox）由宿主在临时工作区提供
//! （`bun/shims/` 下的 JS 文件），扩展目录 symlink 进工作区，Bun 按
//! node_modules 规则解析。SDK 面是有限的、文档化的；Node 内置模块与
//! 第三方依赖由 Bun 原生解决——这是与旧 V8 手写 shim 方案的本质区别（旧方案已删除）。

#![cfg(feature = "bun-runtime")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{oneshot, Mutex, Notify};

use pi_extension_api::{
    create_synthetic_source_info, CommandRegistry, CommandRegistration, ExtensionContext,
    FlagRegistry, HookHandler, HookResult, ShortcutRegistry, SourceInfo, SourceOrigin, SourceScope,
    ToolCallOutput, ToolDefinition, ToolExecuteFn, ToolRegistry,
};

// ============================================================================
// Load result — what a loaded extension factory registered (mirrors
// ExtensionLoadResult; kept local to the bun module
// feature-gated module).
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LoadedToolRecord {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub parameters: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LoadedCommandRecord {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub subcommands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LoadedShortcutRecord {
    pub shortcut: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LoadedFlagRecord {
    pub name: String,
    pub flag_type: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LoadedHandlerRecord {
    pub event: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PendingProviderRegistration {
    pub name: String,
    pub config_json: String,
    pub extension_path: String,
}

#[derive(Debug, Clone, Default)]
pub struct BunLoadResult {
    pub tools: Vec<LoadedToolRecord>,
    pub commands: Vec<LoadedCommandRecord>,
    pub shortcuts: Vec<LoadedShortcutRecord>,
    pub flags: Vec<LoadedFlagRecord>,
    pub handlers: Vec<LoadedHandlerRecord>,
    pub logs: Vec<String>,
    pub pending_providers: Vec<PendingProviderRegistration>,
}

/// Action-method closures, set by `bind_actions()` after the session is
/// created. Mirrors the V8 path's `RuntimeActions` (and TS
/// `ExtensionRunner.bindCore()`): read-actions read the shared state
/// snapshot, write-actions enqueue onto the action bus, `exec` runs a shell
/// command. Before `bind_actions()` the closures are `None` and the
/// corresponding Bun→host requests get an error response (TS
/// `notInitialized`).
#[derive(Default, Clone)]
pub struct BunRuntimeActions {
    pub send_message: Option<Arc<dyn Fn(String, Option<String>) + Send + Sync>>,
    pub send_user_message: Option<Arc<dyn Fn(String, Option<String>) + Send + Sync>>,
    pub append_entry: Option<Arc<dyn Fn(String, Option<String>) + Send + Sync>>,
    pub set_session_name: Option<Arc<dyn Fn(String) + Send + Sync>>,
    pub get_session_name: Option<Arc<dyn Fn() -> Option<String> + Send + Sync>>,
    pub set_label: Option<Arc<dyn Fn(String, Option<String>) + Send + Sync>>,
    pub get_active_tools: Option<Arc<dyn Fn() -> Vec<String> + Send + Sync>>,
    /// Serialized `ToolInfo` objects, matching TS `getAllTools()`.
    pub get_all_tools: Option<Arc<dyn Fn() -> Vec<serde_json::Value> + Send + Sync>>,
    pub set_active_tools: Option<Arc<dyn Fn(Vec<String>) + Send + Sync>>,
    /// Serialized `SlashCommandInfo` objects, matching TS `getCommands()`.
    pub get_commands: Option<Arc<dyn Fn() -> Vec<serde_json::Value> + Send + Sync>>,
    pub get_thinking_level: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    pub set_thinking_level: Option<Arc<dyn Fn(String) + Send + Sync>>,
    /// `provider/model` string, resolved + applied at the next drain point.
    pub set_model: Option<Arc<dyn Fn(String) + Send + Sync>>,
    pub register_provider: Option<Arc<dyn Fn(String, String, String) + Send + Sync>>,
    pub unregister_provider: Option<Arc<dyn Fn(String) + Send + Sync>>,
    /// 执行宿主内置工具（工具工厂 createBashTool 等的 execute 桥接）。
    /// 返回序列化的 `AgentToolResult` 形状。
    pub run_builtin_tool:
        Option<
            Arc<
                dyn Fn(
                        String,
                        Value,
                    ) -> std::pin::Pin<
                        Box<
                            dyn std::future::Future<Output = Result<Value, String>> + Send,
                        >,
                    > + Send
                    + Sync,
            >,
        >,
    /// pi-ai `complete`/`streamSimple` 桥接：宿主解析模型并跑一次补全，
    /// 返回序列化的 `AssistantMessage`。
    pub pi_ai_complete:
        Option<
            Arc<
                dyn Fn(
                        Value,
                    ) -> std::pin::Pin<
                        Box<
                            dyn std::future::Future<Output = Result<Value, String>> + Send,
                        >,
                    > + Send
                    + Sync,
            >,
        >,
    /// pi-ai `getModel` 桥接：返回当前模型 id（`provider/model`）。
    pub get_model: Option<Arc<dyn Fn() -> Option<String> + Send + Sync>>,
}

// ============================================================================
// Embedded assets
// ============================================================================

// 嵌入的 Bun 二进制（xtask `fetch-bun` 下载到 `assets/runtime/`）。
// 文件名按平台：`bun-{os}-{arch}`（darwin-aarch64 / darwin-x64 / linux-* / windows-*）。
// 二进制不进 git（平台相关、61MB）；缺失时 build.rs 生成空切片，
// `ensure_runtime_binary` 在运行时给出清晰错误。
include!(concat!(env!("OUT_DIR"), "/bun_binary.rs"));

/// 嵌入的 bootstrap 脚本（Bun 侧，见 `bootstrap.ts`）。
const BOOTSTRAP_TS: &str = include_str!("bootstrap.ts");

/// 嵌入的真实 SDK 产物（xtask `build-sdk` 从 TS 仓库打包到 `assets/sdk/`）：
/// - `pi-ai-bundle.js` / `pi-coding-agent-bundle.js` / `pi-tui-bundle.js`：
///   TS 源码转译产物（真实实现，非手写 shim）
/// - `typebox/`：真实 typebox npm 包
#[derive(rust_embed::RustEmbed)]
#[folder = "../../assets/sdk/"]
struct SdkAssets;

/// SDK wrapper 文件（bundle + RPC 桥接，见 `sdk_wrappers/`）。
const SDK_WRAPPERS: &[(&str, &str)] = &[
    ("node_modules/@earendil-works/pi-ai/index.js", include_str!("sdk_wrappers/pi-ai.js")),
    ("node_modules/@earendil-works/pi-coding-agent/index.js", include_str!("sdk_wrappers/pi-coding-agent.js")),
    ("node_modules/@earendil-works/pi-tui/index.js", include_str!("sdk_wrappers/pi-tui.js")),
];

/// SDK 包 package.json（Bun 解析入口用）。
const SDK_PACKAGE_JSON: &[(&str, &str)] = &[
    ("node_modules/@earendil-works/pi-ai/package.json", r#"{"name":"@earendil-works/pi-ai","version":"0.84.0","main":"index.js","type":"module"}"#),
    ("node_modules/@earendil-works/pi-coding-agent/package.json", r#"{"name":"@earendil-works/pi-coding-agent","version":"0.84.0","main":"index.js","type":"module"}"#),
    ("node_modules/@earendil-works/pi-tui/package.json", r#"{"name":"@earendil-works/pi-tui","version":"0.84.0","main":"index.js","type":"module"}"#),
];

/// 把嵌入的 SDK 资产写入工作区 node_modules。
fn write_sdk_files(ws: &Path) -> Result<(), String> {
    // bundle 文件
    for (asset, rel) in [
        ("pi-ai-bundle.js", "node_modules/@earendil-works/pi-ai/bundle.js"),
        ("pi-coding-agent-bundle.js", "node_modules/@earendil-works/pi-coding-agent/bundle.js"),
        ("pi-tui-bundle.js", "node_modules/@earendil-works/pi-tui/bundle.js"),
    ] {
        let data = SdkAssets::get(asset).ok_or_else(|| format!("missing SDK asset: {asset} (run `cargo run -p xtask -- build-sdk`)"))?;
        let path = ws.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, data.data.as_ref())
            .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
    }
    // typebox 真实包（整个目录）
    for file in SdkAssets::iter() {
        let name = file.as_ref();
        if name.starts_with("typebox/") {
            let data = SdkAssets::get(name).ok_or_else(|| format!("missing typebox asset: {name}"))?;
            let path = ws.join("node_modules").join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
            }
            std::fs::write(&path, data.data.as_ref())
                .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
        }
    }
    // wrapper + package.json
    for (rel, content) in SDK_WRAPPERS.iter().chain(SDK_PACKAGE_JSON.iter()) {
        let path = ws.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, content)
            .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
    }
    Ok(())
}

// ============================================================================
// Runtime binary management
// ============================================================================

/// 解压嵌入的 Bun 二进制到 `{agent_dir}/runtime/bun`（带版本缓存，幂等）。
fn ensure_runtime_binary(agent_dir: &Path) -> Result<PathBuf, String> {
    if BUN_BINARY.is_empty() {
        return Err(
            "Bun runtime binary is not embedded. Run `cargo run -p xtask -- fetch-bun` (or `make fetch-bun`) to download it into assets/runtime/.".to_string(),
        );
    }
    let runtime_dir = agent_dir.join("runtime");
    let target = runtime_dir.join("bun");
    if target.exists() {
        return Ok(target);
    }
    std::fs::create_dir_all(&runtime_dir)
        .map_err(|e| format!("Failed to create {}: {e}", runtime_dir.display()))?;
    std::fs::write(&target, BUN_BINARY)
        .map_err(|e| format!("Failed to write bun binary: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Failed to chmod bun binary: {e}"))?;
    }
    Ok(target)
}

/// 从扩展入口文件向上找包根，返回 (root, 相对入口)。
///
/// 只有**声明了 `pi.extensions` 的 package.json** 才算包根（镜像 TS
/// `resolveExtensionEntries`：普通 package.json 不参与判定，避免把入口文件
/// 祖先目录里无关的 package.json 误当包根）。找不到则 root = 入口文件所在目录。
fn find_extension_root(entry: &Path) -> (PathBuf, String) {
    let mut dir = entry.parent().unwrap_or(Path::new(".")).to_path_buf();
    loop {
        if let Ok(content) = std::fs::read_to_string(dir.join("package.json")) {
            if let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&content) {
                let has_pi_extensions = pkg
                    .get("pi")
                    .and_then(|pi| pi.get("extensions"))
                    .and_then(|e| e.as_array())
                    .is_some_and(|arr| !arr.is_empty());
                if has_pi_extensions {
                    let rel = entry
                        .strip_prefix(&dir)
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|_| entry.to_string_lossy().to_string());
                    return (dir, rel);
                }
            }
        }
        if !dir.pop() {
            break;
        }
    }
    let parent = entry.parent().unwrap_or(Path::new("."));
    (
        parent.to_path_buf(),
        entry
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| entry.to_string_lossy().to_string()),
    )
}

/// 创建临时工作区：node_modules shim + 扩展目录 symlink + bootstrap.ts。
/// 目录名带随机后缀，避免同一进程内并行 runner 互相覆盖。
fn create_workspace(agent_dir: &Path, extension_path: &Path) -> Result<PathBuf, String> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let ws = agent_dir
        .join("runtime")
        .join(format!("ws-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(&ws)
        .map_err(|e| format!("Failed to create workspace: {e}"))?;

    // 真实 SDK → node_modules（bundle + wrapper + typebox）
    write_sdk_files(&ws)?;

    // 扩展目录复制进工作区（Bun 会把 symlink 解析到真实路径，导致扩展的
    // import 从真实路径解析、找不到工作区 node_modules 的 shims——尤其
    // /tmp → /private/tmp 这类 canonicalize 场景）。复制源码 + symlink
    // 扩展自身的 node_modules（第三方依赖），保证解析确定。
    let (root, _rel) = find_extension_root(extension_path);
    let ext_dir = ws.join("ext");
    std::fs::create_dir_all(&ext_dir)
        .map_err(|e| format!("Failed to create ext dir: {e}"))?;
    copy_dir_contents(&root, &ext_dir).map_err(|e| format!("Failed to copy extension: {e}"))?;
    // 扩展自身的 node_modules（第三方依赖）symlink 进复制树。
    let src_nm = root.join("node_modules");
    if src_nm.is_dir() {
        let dst_nm = ext_dir.join("node_modules");
        let _ = std::fs::remove_dir_all(&dst_nm);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&src_nm, &dst_nm)
            .map_err(|e| format!("Failed to symlink extension node_modules: {e}"))?;
        #[cfg(not(unix))]
        copy_dir(&src_nm, &dst_nm).map_err(|e| format!("Failed to copy extension node_modules: {e}"))?;
    }

    // bootstrap.ts
    std::fs::write(ws.join("bootstrap.ts"), BOOTSTRAP_TS)
        .map_err(|e| format!("Failed to write bootstrap: {e}"))?;

    Ok(ws)
}

#[cfg(not(unix))]
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// 复制目录内容到目标（跳过 `node_modules`——它单独 symlink，避免复制
/// 第三方依赖的庞大体积）。
fn copy_dir_contents(src: &Path, dst: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == "node_modules" {
            continue;
        }
        let target = dst.join(&name);
        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_dir_contents(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)
                .map_err(|e| std::io::Error::new(e.kind(), format!("copy {}: {e}", entry.path().display())))?;
        }
    }
    Ok(())
}

// ============================================================================
// BunExtensionRunner — spawn + RPC
// ============================================================================

/// 与 Bun 子进程的 RPC 会话。`request` 是 `&self`（内部 Mutex），
/// 多个 adapter 可并发调用。
///
/// RPC 协议（newline-delimited JSON）：
/// - 宿主 → Bun 请求：`{"id": <HOST_ID_BASE+n>, "method": ..., "params": ...}`
/// - Bun → 宿主请求：`{"id": <小整数>, "method": ..., "params": ...}`
/// - 响应：`{"id": <对应 id>, "result": ...}` 或 `{"error": ...}`
/// - Bun → 宿主通知：`{"method": ..., "params": ...}`（无 id）
///
/// id 空间隔离：宿主请求 id 从 `HOST_ID_BASE` 起，Bun 侧请求 id 从 1 起，
/// 两侧永不冲突（修复了早期两侧都从 1 起导致请求/响应互相误认的 bug）。
///
/// Bun → 宿主请求（action 方法：sendMessage/exec/getCommands/...）由
/// `read_stdout` 分发给 `bind_actions()` 安装的闭包，并回写响应。
pub struct BunExtensionRunner {
    child: std::sync::Mutex<Child>,
    stdin: Arc<Mutex<tokio::io::BufWriter<ChildStdin>>>,
    stdout_task: tokio::task::JoinHandle<()>,
    next_id: std::sync::atomic::AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>,
    load_result: Arc<Mutex<BunLoadResult>>,
    /// Action-method closures（bind_actions 后可用）。
    actions: Arc<Mutex<Option<BunRuntimeActions>>>,
    /// 扩展 flag 值（CLI 传入覆盖 registerFlag 默认值）。
    flags: Arc<Mutex<HashMap<String, String>>>,
}

/// 宿主请求 id 基址（Bun 侧请求 id 从 1 起，两侧永不冲突）。
const HOST_ID_BASE: u64 = 1_000_000_000;

impl BunExtensionRunner {
    /// Spawn Bun 子进程并等待扩展 factory 完成（"loaded" 握手）。
    pub async fn spawn(
        extension_path: &Path,
        cwd: &Path,
        agent_dir: &Path,
        flags: &std::collections::HashMap<String, String>,
    ) -> Result<Self, String> {
        let bun = ensure_runtime_binary(agent_dir)?;
        let ws = create_workspace(agent_dir, extension_path)?;
        let (root, rel) = find_extension_root(extension_path);
        let entry = format!("./ext/{}", rel);

        let mut child = tokio::process::Command::new(&bun)
            .args(["run", "bootstrap.ts", &entry])
            .current_dir(&ws)
            .env("PI_EXTENSION_PATH", root.to_string_lossy().to_string())
            .env("PI_RS_HOME", agent_dir.to_string_lossy().to_string())
            .env("PI_SESSION_CWD", cwd.to_string_lossy().to_string())
            .env(
                "PI_EXTENSION_FLAGS",
                serde_json::to_string(flags).unwrap_or_else(|_| "{}".to_string()),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("Failed to spawn bun: {e}"))?;

        let stdin = child.stdin.take().ok_or("bun stdin missing")?;
        let stdout = child.stdout.take().ok_or("bun stdout missing")?;

        let pending = Arc::new(Mutex::new(HashMap::new()));
        let load_result = Arc::new(Mutex::new(BunLoadResult::default()));
        let loaded = Arc::new(Notify::new());
        let actions = Arc::new(Mutex::new(None));
        let flags = Arc::new(Mutex::new(HashMap::new()));
        let stdin = Arc::new(Mutex::new(tokio::io::BufWriter::new(stdin)));

        let runner = Self {
            child: std::sync::Mutex::new(child),
            stdin: stdin.clone(),
            stdout_task: tokio::task::spawn(Self::read_stdout(
                stdout,
                stdin,
                pending.clone(),
                load_result.clone(),
                loaded.clone(),
                actions.clone(),
                flags.clone(),
            )),
            next_id: std::sync::atomic::AtomicU64::new(0),
            pending,
            load_result,
            actions,
            flags,
        };

        // 等待 factory 完成（超时保护）。
        tokio::time::timeout(std::time::Duration::from_secs(30), loaded.notified())
            .await
            .map_err(|_| "Timed out waiting for extension to load in Bun".to_string())?;

        Ok(runner)
    }

    /// stdout 读取任务：解析 JSON 行。
    /// - `id` 匹配宿主 pending → 宿主请求的响应
    /// - `id` 不匹配 + 有 `method` → Bun → 宿主请求（action 方法），分发后回写响应
    /// - 无 `id` + 有 `method` → 通知（register_*/log/loaded）
    async fn read_stdout(
        stdout: ChildStdout,
        stdin: Arc<Mutex<tokio::io::BufWriter<ChildStdin>>>,
        pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>,
        load_result: Arc<Mutex<BunLoadResult>>,
        loaded: Arc<Notify>,
        actions: Arc<Mutex<Option<BunRuntimeActions>>>,
        flags: Arc<Mutex<HashMap<String, String>>>,
    ) {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let id = msg.get("id").and_then(|v| v.as_u64());
            let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
            let params = msg.get("params").cloned().unwrap_or(Value::Null);

            // 1. 宿主请求的响应？
            if let Some(id) = id {
                if let Some(sender) = pending.lock().await.remove(&id) {
                    if let Some(err) = msg.get("error") {
                        let _ = sender.send(Err(err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown error")
                            .to_string()));
                    } else {
                        let _ = sender.send(Ok(msg.get("result").cloned().unwrap_or(Value::Null)));
                    }
                    continue;
                }
                // 2. Bun → 宿主请求（action 方法）？
                if !method.is_empty() {
                    let response = Self::handle_bun_request(method, &params, &actions, &flags).await;
                    let reply = match response {
                        Ok(result) => serde_json::json!({ "id": id, "result": result }),
                        Err(e) => serde_json::json!({ "id": id, "error": { "message": e } }),
                    };
                    let mut line = serde_json::to_string(&reply).unwrap_or_default();
                    line.push('\n');
                    let mut w = stdin.lock().await;
                    let _ = w.write_all(line.as_bytes()).await;
                    let _ = w.flush().await;
                    continue;
                }
                continue;
            }

            // 3. 通知
            let mut lr = load_result.lock().await;
            match method {
                "register_tool" => lr.tools.push(LoadedToolRecord {
                    name: params.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    description: params.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    parameters: params.get("parameters").and_then(|v| v.as_str()).map(|s| s.to_string()),
                }),
                "register_command" => lr.commands.push(LoadedCommandRecord {
                    name: params.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    description: params.get("description").and_then(|v| v.as_str()).map(|s| s.to_string()),
                    subcommands: params
                        .get("subcommands")
                        .and_then(|v| v.as_str())
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_default(),
                }),
                "register_shortcut" => lr.shortcuts.push(LoadedShortcutRecord {
                    shortcut: params.get("shortcut").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    description: params.get("description").and_then(|v| v.as_str()).map(|s| s.to_string()),
                }),
                "register_flag" => lr.flags.push(LoadedFlagRecord {
                    name: params.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    flag_type: params.get("flag_type").and_then(|v| v.as_str()).unwrap_or("boolean").to_string(),
                    description: params.get("description").and_then(|v| v.as_str()).map(|s| s.to_string()),
                    default_value: params.get("default_value").and_then(|v| v.as_str()).map(|s| s.to_string()),
                }),
                "register_handler" => lr.handlers.push(LoadedHandlerRecord {
                    event: params.get("event").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                }),
                "register_provider" => lr.pending_providers.push(PendingProviderRegistration {
                    name: params.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    config_json: params.get("config_json").and_then(|v| v.as_str()).unwrap_or("{}").to_string(),
                    extension_path: params.get("extension_path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                }),
                "log" => lr.logs.push(params.get("message").and_then(|v| v.as_str()).unwrap_or("").to_string()),
                "loaded" => loaded.notify_one(),
                _ => {}
            }
        }
    }

    /// 处理 Bun → 宿主请求（action 方法），返回响应值。
    async fn handle_bun_request(
        method: &str,
        params: &Value,
        actions: &Arc<Mutex<Option<BunRuntimeActions>>>,
        flags: &Arc<Mutex<HashMap<String, String>>>,
    ) -> Result<Value, String> {
        let actions = actions.lock().await.clone().unwrap_or_default();
        let str_param = |k: &str| params.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());
        let str_or_null = |k: &str| params.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());

        match method {
            "send_message" => {
                let f = actions.send_message.as_ref().ok_or("sendMessage: runtime not initialized")?;
                f(str_param("message_json").unwrap_or_default(), str_or_null("options_json"));
                Ok(Value::Null)
            }
            "send_user_message" => {
                let f = actions.send_user_message.as_ref().ok_or("sendUserMessage: runtime not initialized")?;
                f(str_param("content").unwrap_or_default(), str_or_null("options_json"));
                Ok(Value::Null)
            }
            "append_entry" => {
                let f = actions.append_entry.as_ref().ok_or("appendEntry: runtime not initialized")?;
                f(str_param("custom_type").unwrap_or_default(), str_or_null("data_json"));
                Ok(Value::Null)
            }
            "set_session_name" => {
                let f = actions.set_session_name.as_ref().ok_or("setSessionName: runtime not initialized")?;
                f(str_param("name").unwrap_or_default());
                Ok(Value::Null)
            }
            "get_session_name" => {
                let f = actions.get_session_name.as_ref().ok_or("getSessionName: runtime not initialized")?;
                Ok(f().map(Value::String).unwrap_or(Value::Null))
            }
            "set_label" => {
                let f = actions.set_label.as_ref().ok_or("setLabel: runtime not initialized")?;
                f(str_param("entry_id").unwrap_or_default(), str_or_null("label"));
                Ok(Value::Null)
            }
            "get_active_tools" => {
                let f = actions.get_active_tools.as_ref().ok_or("getActiveTools: runtime not initialized")?;
                Ok(serde_json::to_value(f()).unwrap_or(Value::Array(vec![])))
            }
            "get_all_tools" => {
                let f = actions.get_all_tools.as_ref().ok_or("getAllTools: runtime not initialized")?;
                Ok(serde_json::to_value(f()).unwrap_or(Value::Array(vec![])))
            }
            "set_active_tools" => {
                let f = actions.set_active_tools.as_ref().ok_or("setActiveTools: runtime not initialized")?;
                let tools: Vec<String> = params
                    .get("tools_json")
                    .and_then(|v| v.as_str())
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();
                f(tools);
                Ok(Value::Null)
            }
            "get_commands" => {
                let f = actions.get_commands.as_ref().ok_or("getCommands: runtime not initialized")?;
                Ok(serde_json::to_value(f()).unwrap_or(Value::Array(vec![])))
            }
            "get_thinking_level" => {
                let f = actions.get_thinking_level.as_ref().ok_or("getThinkingLevel: runtime not initialized")?;
                Ok(Value::String(f()))
            }
            "set_thinking_level" => {
                let f = actions.set_thinking_level.as_ref().ok_or("setThinkingLevel: runtime not initialized")?;
                f(str_param("level").unwrap_or_default());
                Ok(Value::Null)
            }
            "set_model" => {
                let f = actions.set_model.as_ref().ok_or("setModel: runtime not initialized")?;
                f(str_param("model_id").unwrap_or_default());
                Ok(Value::Null)
            }
            "register_provider" => {
                let f = actions.register_provider.as_ref().ok_or("registerProvider: runtime not initialized")?;
                f(
                    str_param("name").unwrap_or_default(),
                    str_param("config_json").unwrap_or_else(|| "{}".to_string()),
                    str_param("extension_path").unwrap_or_default(),
                );
                Ok(Value::Null)
            }
            "unregister_provider" => {
                let f = actions.unregister_provider.as_ref().ok_or("unregisterProvider: runtime not initialized")?;
                f(str_param("name").unwrap_or_default());
                Ok(Value::Null)
            }
            "exec" => {
                let command = str_param("command").unwrap_or_default();
                let args: Vec<String> = params
                    .get("args_json")
                    .and_then(|v| v.as_str())
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();
                let options_json = str_or_null("options_json");
                run_exec(&command, &args, options_json.as_deref()).await
            }
            "run_builtin_tool" => {
                let f = actions
                    .run_builtin_tool
                    .as_ref()
                    .ok_or("runBuiltinTool: runtime not initialized")?;
                let name = str_param("name").unwrap_or_default();
                let params: Value = params
                    .get("params_json")
                    .and_then(|v| v.as_str())
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or(Value::Null);
                f(name, params).await
            }
            "pi_ai_complete" => {
                let f = actions
                    .pi_ai_complete
                    .as_ref()
                    .ok_or("pi-ai complete: runtime not initialized")?;
                let request: Value = params
                    .get("request_json")
                    .and_then(|v| v.as_str())
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or(Value::Null);
                f(request).await
            }
            "get_model" => {
                let f = actions.get_model.as_ref().ok_or("getModel: runtime not initialized")?;
                Ok(f().map(Value::String).unwrap_or(Value::Null))
            }
            "get_flag" => {
                let name = str_param("name").unwrap_or_default();
                Ok(flags
                    .lock()
                    .await
                    .get(&name)
                    .cloned()
                    .map(Value::String)
                    .unwrap_or(Value::Null))
            }
            "set_flag" => {
                let name = str_param("name").unwrap_or_default();
                let value = str_param("value").unwrap_or_default();
                flags.lock().await.insert(name, value);
                Ok(Value::Null)
            }
            _ => Err(format!("unknown action method: {method}")),
        }
    }

    /// 发送 RPC 请求并等待响应。宿主请求 id 从 `HOST_ID_BASE` 起，
    /// 与 Bun 侧请求 id（小整数）永不冲突。
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = HOST_ID_BASE + self.next_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let line = serde_json::json!({ "id": id, "method": method, "params": params });
        let mut line = serde_json::to_string(&line).map_err(|e| format!("serialize: {e}"))?;
        line.push('\n');
        self.stdin
            .lock()
            .await
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("write to bun: {e}"))?;
        self.stdin.lock().await.flush().await.map_err(|e| format!("flush: {e}"))?;
        rx.await.map_err(|_| "bun process closed the channel".to_string())?
    }

    /// 安装 action-method 闭包（session 创建后调用，镜像 TS `bindCore()`）。
    /// 之后 Bun → 宿主的 action 请求（sendMessage/exec/getCommands/...）
    /// 由 `read_stdout` 分发到这些闭包。
    pub async fn bind_actions(&self, actions: BunRuntimeActions) {
        *self.actions.lock().await = Some(actions);
    }

    /// 设置扩展 flag 值（CLI 传入覆盖 registerFlag 默认值）。
    pub async fn set_flag_value(&self, name: &str, value: &str) {
        self.flags.lock().await.insert(name.to_string(), value.to_string());
    }

    /// 同步设置 flag（best-effort，供同步回调用）。
    pub fn set_flag_value_sync(&self, name: &str, value: &str) {
        let mut flags = self.flags.blocking_lock();
        flags.insert(name.to_string(), value.to_string());
    }

    /// 执行扩展注册的 JS 工具。
    pub async fn execute_tool(
        &self,
        name: &str,
        call_id: &str,
        params: Value,
    ) -> Result<ToolCallOutput, String> {
        let result = self
            .request(
                "execute_tool",
                serde_json::json!({ "name": name, "callId": call_id, "params": params }),
            )
            .await?;
        // JS 侧返回 AgentToolResult 形状（content/details/terminate）。
        let content = result.get("content").cloned().unwrap_or_else(|| Value::Array(vec![]));
        let content = content.as_array().cloned().unwrap_or_default();
        Ok(ToolCallOutput {
            content,
            details: result.get("details").cloned(),
            is_error: false,
            terminate: result.get("terminate").and_then(|t| t.as_bool()),
        })
    }

    /// 触发事件，返回 handler 返回值数组（result-bearing hook 用）。
    pub async fn fire_event(&self, event: &str, data: Value) -> Result<Value, String> {
        self.request(
            "fire_event",
            serde_json::json!({ "event": event, "data": data }),
        )
        .await
    }

    /// 执行扩展注册的命令 handler。
    pub async fn execute_command(&self, name: &str) -> Result<(), String> {
        self.request("execute_command", serde_json::json!({ "name": name }))
            .await
            .map(|_| ())
    }

    /// 取出加载结果（factory 阶段收集的注册信息）。
    pub async fn take_load_result(&self) -> BunLoadResult {
        std::mem::take(&mut *self.load_result.lock().await)
    }

    /// 关闭子进程。
    pub async fn shutdown(&self) {
        let _ = self.request("shutdown", Value::Null).await;
        let _ = self.child.lock().unwrap_or_else(std::sync::PoisonError::into_inner).start_kill();
        self.stdout_task.abort();
    }
}

impl Drop for BunExtensionRunner {
    fn drop(&mut self) {
        let _ = self.child.lock().unwrap_or_else(std::sync::PoisonError::into_inner).start_kill();
        self.stdout_task.abort();
    }
}

/// 执行 shell 命令，返回 TS `ExecResult` 形状（`{stdout, stderr, code, killed}`）。
/// 支持 timeout/cwd 选项。
async fn run_exec(
    command: &str,
    args: &[String],
    options_json: Option<&str>,
) -> Result<Value, String> {
    let options: Value = options_json
        .and_then(|o| serde_json::from_str(o).ok())
        .unwrap_or(Value::Null);
    let timeout_ms = options.get("timeout").and_then(|t| t.as_u64());
    let cwd_override = options.get("cwd").and_then(|c| c.as_str()).map(String::from);

    let mut cmd = tokio::process::Command::new(command);
    cmd.args(args);
    if let Some(c) = &cwd_override {
        cmd.current_dir(c);
    }

    let output = if let Some(ms) = timeout_ms {
        match tokio::time::timeout(std::time::Duration::from_millis(ms), cmd.output()).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => return Err(format!("spawn failed: {e}")),
            Err(_) => {
                return Ok(serde_json::json!({
                    "stdout": "",
                    "stderr": "command timed out",
                    "code": 1,
                    "killed": true,
                }));
            }
        }
    } else {
        cmd.output().await.map_err(|e| format!("spawn failed: {e}"))?
    };

    Ok(serde_json::json!({
        "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
        "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
        "code": output.status.code().unwrap_or(1),
        "killed": false,
    }))
}

// ============================================================================
// BunExtensionAdapter — HookHandler impl（镜像 JsExtensionAdapter）
// ============================================================================

/// 一个已加载的 Bun 扩展的 HookHandler 适配器。
pub struct BunExtensionAdapter {
    name: String,
    load_result: BunLoadResult,
    source_info: SourceInfo,
    runner: Arc<BunExtensionRunner>,
}

impl BunExtensionAdapter {
    #[must_use]
    pub fn new(
        extension_path: &str,
        load_result: BunLoadResult,
        runner: Arc<BunExtensionRunner>,
    ) -> Self {
        let name = Path::new(extension_path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| extension_path.to_string());
        let source_info = create_synthetic_source_info(
            extension_path.to_string(),
            "extension".to_string(),
            Some(SourceScope::Project),
            Some(SourceOrigin::TopLevel),
            None,
        );
        Self {
            name,
            load_result,
            source_info,
            runner,
        }
    }

    #[must_use]
    pub fn source_info(&self) -> &SourceInfo {
        &self.source_info
    }

    #[must_use]
    pub fn pending_providers(&self) -> &[PendingProviderRegistration] {
        &self.load_result.pending_providers
    }

    fn has_handler(&self, event: &str) -> bool {
        self.load_result.handlers.iter().any(|h| h.event == event)
    }
}

#[async_trait::async_trait]
impl HookHandler for BunExtensionAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn register_tools(&self, tools: &mut ToolRegistry) {
        for tool in &self.load_result.tools {
            let runner = self.runner.clone();
            let tool_name = tool.name.clone();
            let execute: ToolExecuteFn = Arc::new(move |call_id, params, _signal| {
                let runner = runner.clone();
                let tool_name = tool_name.clone();
                Box::pin(async move {
                    runner
                        .execute_tool(&tool_name, &call_id, params)
                        .await
                        .map_err(|e| Box::<dyn std::error::Error + Send + Sync>::from(e))
                })
            });
            let parameters = tool
                .parameters
                .as_ref()
                .and_then(|p| serde_json::from_str(p).ok());
            tools.register(
                &tool.name,
                ToolDefinition {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    parameters,
                    execute: Some(execute),
                    ..Default::default()
                },
            );
        }
    }

    fn register_commands(&self, commands: &mut CommandRegistry) {
        for cmd in &self.load_result.commands {
            let runner = self.runner.clone();
            let cmd_name = cmd.name.clone();
            let execute: Arc<
                dyn Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
                    + Send
                    + Sync,
            > = Arc::new(move |_arg: String| {
                let runner = runner.clone();
                let cmd_name = cmd_name.clone();
                Box::pin(async move {
                    let _ = runner.execute_command(&cmd_name).await;
                })
            });
            commands.register(
                &cmd.name,
                CommandRegistration {
                    description: cmd.description.clone().unwrap_or_default(),
                    execute,
                    get_argument_completions: None,
                },
            );
        }
    }

    fn register_shortcuts(&self, shortcuts: &mut ShortcutRegistry) {
        for sc in &self.load_result.shortcuts {
            shortcuts.register(&sc.shortcut, sc.description.as_deref().unwrap_or(""));
        }
    }

    fn register_flags(&self, flags: &mut FlagRegistry) {
        for flag in &self.load_result.flags {
            flags.register(&flag.name, &flag.description.clone().unwrap_or_default());
        }
    }

    async fn handle_tool_call(
        &self,
        tool_name: &str,
        params: Value,
        _ctx: &ExtensionContext,
    ) -> Option<ToolCallOutput> {
        if !self.load_result.tools.iter().any(|t| t.name == tool_name) {
            return None;
        }
        match self.runner.execute_tool(tool_name, "bun-tool-call", params).await {
            Ok(output) => Some(output),
            Err(e) => Some(ToolCallOutput {
                content: vec![Value::String(format!("Bun tool execution error: {e}"))],
                details: None,
                is_error: true,
                terminate: None,
            }),
        }
    }

    // ── 事件钩子：委托给 Bun 侧 handler ──────────────────────────────

    async fn on_session_start(&self, reason: &str, previous_session_file: Option<&str>) {
        if self.has_handler("session_start") {
            let _ = self
                .runner
                .fire_event(
                    "session_start",
                    serde_json::json!({ "reason": reason, "previousSessionFile": previous_session_file }),
                )
                .await;
        }
    }

    async fn on_session_shutdown(&self, reason: &str, target_session_file: Option<&str>) {
        if self.has_handler("session_shutdown") {
            let _ = self
                .runner
                .fire_event(
                    "session_shutdown",
                    serde_json::json!({ "reason": reason, "targetSessionFile": target_session_file }),
                )
                .await;
        }
    }

    async fn on_agent_start(&self) {
        if self.has_handler("agent_start") {
            let _ = self.runner.fire_event("agent_start", serde_json::json!({})).await;
        }
    }

    async fn on_agent_end(&self, messages: &[Value]) {
        if self.has_handler("agent_end") {
            let _ = self
                .runner
                .fire_event("agent_end", serde_json::json!({ "messages": messages }))
                .await;
        }
    }

    async fn on_agent_settled(&self) {
        if self.has_handler("agent_settled") {
            let _ = self.runner.fire_event("agent_settled", serde_json::json!({})).await;
        }
    }

    async fn on_turn_start(&self, turn_index: u32) {
        if self.has_handler("turn_start") {
            let _ = self
                .runner
                .fire_event("turn_start", serde_json::json!({ "turnIndex": turn_index }))
                .await;
        }
    }

    async fn on_turn_end(&self, turn_index: u32, message: &Value, tool_results: &[Value]) {
        if self.has_handler("turn_end") {
            let _ = self
                .runner
                .fire_event(
                    "turn_end",
                    serde_json::json!({ "turnIndex": turn_index, "message": message, "toolResults": tool_results }),
                )
                .await;
        }
    }

    async fn on_message_start(&self, message: &Value) {
        if self.has_handler("message_start") {
            let _ = self
                .runner
                .fire_event("message_start", serde_json::json!({ "message": message }))
                .await;
        }
    }

    async fn on_message_update(&self, message: &Value) {
        if self.has_handler("message_update") {
            let _ = self
                .runner
                .fire_event("message_update", serde_json::json!({ "message": message }))
                .await;
        }
    }

    async fn on_message_end(&self, message: &Value) {
        if self.has_handler("message_end") {
            let _ = self
                .runner
                .fire_event("message_end", serde_json::json!({ "message": message }))
                .await;
        }
    }

    async fn on_model_select(&self, model: &str, previous_model: Option<&str>) {
        if self.has_handler("model_select") {
            let _ = self
                .runner
                .fire_event(
                    "model_select",
                    serde_json::json!({ "model": model, "previousModel": previous_model }),
                )
                .await;
        }
    }

    async fn on_compact(&self, summary: &str, tokens_before: u64) {
        if self.has_handler("session_compact") {
            let _ = self
                .runner
                .fire_event(
                    "session_compact",
                    serde_json::json!({ "summary": summary, "tokensBefore": tokens_before }),
                )
                .await;
        }
    }

    async fn on_tool_execution_start(&self, tool_call_id: &str, tool_name: &str, args: &Value) {
        if self.has_handler("tool_execution_start") {
            let _ = self
                .runner
                .fire_event(
                    "tool_execution_start",
                    serde_json::json!({ "toolCallId": tool_call_id, "toolName": tool_name, "input": args }),
                )
                .await;
        }
    }

    async fn on_tool_execution_end(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        result: &Value,
        is_error: bool,
    ) {
        if self.has_handler("tool_execution_end") {
            let _ = self
                .runner
                .fire_event(
                    "tool_execution_end",
                    serde_json::json!({
                        "toolCallId": tool_call_id,
                        "toolName": tool_name,
                        "result": result,
                        "isError": is_error,
                    }),
                )
                .await;
        }
    }

    async fn before_tool_call(&self, tool_name: String, args: Value) -> HookResult<(String, Value)> {
        if self.has_handler("tool_call") {
            let data = serde_json::json!({ "toolName": tool_name, "input": args });
            if let Ok(results) = self.runner.fire_event("tool_call", data).await {
                if let Some(blocked) = results.as_array().and_then(|arr| {
                    arr.iter().find_map(|r| {
                        let is_block = r.get("block").and_then(|b| b.as_bool()).unwrap_or(false);
                        is_block.then_some(
                            r.get("reason")
                                .and_then(|x| x.as_str())
                                .unwrap_or("blocked by extension")
                                .to_string(),
                        )
                    })
                }) {
                    return HookResult::Cancel(blocked);
                }
            }
        }
        HookResult::Continue((tool_name, args))
    }

    async fn after_tool_call(&self, tool_name: &str, result: &Value, is_error: bool) -> HookResult<()> {
        if self.has_handler("tool_result") {
            let _ = self
                .runner
                .fire_event(
                    "tool_result",
                    serde_json::json!({ "toolName": tool_name, "result": result, "isError": is_error }),
                )
                .await;
        }
        HookResult::Continue(())
    }

    async fn on_session_info_changed(&self, name: Option<&str>) {
        if self.has_handler("session_info_changed") {
            let _ = self
                .runner
                .fire_event("session_info_changed", serde_json::json!({ "name": name }))
                .await;
        }
    }

    async fn on_thinking_level_select(&self, level: &str, previous_level: &str) {
        if self.has_handler("thinking_level_select") {
            let _ = self
                .runner
                .fire_event(
                    "thinking_level_select",
                    serde_json::json!({ "level": level, "previousLevel": previous_level }),
                )
                .await;
        }
    }

    async fn on_tree(&self, new_leaf_id: Option<&str>, old_leaf_id: Option<&str>) {
        if self.has_handler("session_tree") {
            let _ = self
                .runner
                .fire_event(
                    "session_tree",
                    serde_json::json!({ "newLeafId": new_leaf_id, "oldLeafId": old_leaf_id }),
                )
                .await;
        }
    }

    async fn on_resources_discover(
        &self,
        cwd: &str,
        reason: &str,
    ) -> Option<pi_extension_api::ResourcesDiscoverResult> {
        if !self.has_handler("resources_discover") {
            return None;
        }
        let data = serde_json::json!({ "cwd": cwd, "reason": reason });
        if let Ok(results) = self.runner.fire_event("resources_discover", data).await {
            if let Some(r) = results.as_array().and_then(|arr| arr.iter().find(|r| !r.is_null())) {
                return serde_json::from_value(r.clone()).ok();
            }
        }
        None
    }

    async fn on_project_trust(&self, cwd: &str) -> Option<pi_extension_api::ProjectTrustResult> {
        if !self.has_handler("project_trust") {
            return None;
        }
        let data = serde_json::json!({ "cwd": cwd });
        if let Ok(results) = self.runner.fire_event("project_trust", data).await {
            if let Some(r) = results.as_array().and_then(|arr| arr.iter().find(|r| !r.is_null())) {
                return serde_json::from_value(r.clone()).ok();
            }
        }
        None
    }

    async fn on_user_bash(&self, command: &str, cwd: &str) -> Option<pi_extension_api::UserBashResult> {
        if !self.has_handler("user_bash") {
            return None;
        }
        let data = serde_json::json!({ "command": command, "cwd": cwd });
        if let Ok(results) = self.runner.fire_event("user_bash", data).await {
            if let Some(r) = results.as_array().and_then(|arr| arr.iter().find(|r| !r.is_null())) {
                return serde_json::from_value(r.clone()).ok();
            }
        }
        None
    }

    async fn on_context(&self, messages: &[Value]) {
        if self.has_handler("context") {
            let _ = self
                .runner
                .fire_event("context", serde_json::json!({ "messages": messages }))
                .await;
        }
    }

    async fn on_context_mut(&self, messages: Vec<Value>) -> HookResult<Vec<Value>> {
        if !self.has_handler("context") {
            return HookResult::Continue(messages);
        }
        let data = serde_json::json!({ "messages": messages });
        if let Ok(results) = self.runner.fire_event("context", data).await {
            if let Some(r) = results.as_array().and_then(|arr| arr.iter().find(|r| !r.is_null())) {
                let new_messages = r
                    .get("messages")
                    .and_then(|m| serde_json::from_value::<Vec<Value>>(m.clone()).ok())
                    .or_else(|| r.as_array().cloned());
                if let Some(m) = new_messages {
                    return HookResult::Continue(m);
                }
            }
        }
        HookResult::Continue(messages)
    }

    async fn on_tool_result_mut(
        &self,
        tool_name: &str,
        content: Vec<Value>,
        details: Option<Value>,
        is_error: bool,
    ) -> HookResult<(Vec<Value>, Option<Value>, bool)> {
        if !self.has_handler("tool_result") {
            return HookResult::Continue((content, details, is_error));
        }
        let data = serde_json::json!({
            "toolName": tool_name,
            "content": content,
            "details": details,
            "isError": is_error,
        });
        if let Ok(results) = self.runner.fire_event("tool_result", data).await {
            if let Some(r) = results.as_array().and_then(|arr| arr.iter().find(|r| !r.is_null())) {
                if let Some(content) = r
                    .get("content")
                    .cloned()
                    .and_then(|v| serde_json::from_value::<Vec<Value>>(v).ok())
                {
                    let is_error = r
                        .get("isError")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(is_error);
                    return HookResult::Continue((content, r.get("details").cloned(), is_error));
                }
            }
        }
        HookResult::Continue((content, details, is_error))
    }

    async fn before_agent_start(
        &self,
        prompt: String,
        images: Option<Vec<Value>>,
        system_prompt: String,
        system_prompt_options: Option<Value>,
    ) -> HookResult<(String, String, Option<Vec<Value>>)> {
        if !self.has_handler("before_agent_start") {
            return HookResult::Continue((prompt, system_prompt, None));
        }
        let data = serde_json::json!({
            "prompt": prompt,
            "images": images,
            "systemPrompt": system_prompt,
            "systemPromptOptions": system_prompt_options,
        });
        if let Ok(results) = self.runner.fire_event("before_agent_start", data).await {
            if let Some(r) = results.as_array().and_then(|arr| arr.iter().find(|r| !r.is_null())) {
                let prompt = r
                    .get("prompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&prompt)
                    .to_string();
                let system_prompt = r
                    .get("systemPrompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&system_prompt)
                    .to_string();
                let images = r
                    .get("images")
                    .cloned()
                    .and_then(|v| serde_json::from_value::<Vec<Value>>(v).ok());
                return HookResult::Continue((prompt, system_prompt, images));
            }
        }
        HookResult::Continue((prompt, system_prompt, None))
    }

    async fn on_input(
        &self,
        text: String,
        images: Option<Vec<Value>>,
        source: String,
        streaming_behavior: Option<String>,
    ) -> HookResult<String> {
        if !self.has_handler("input") {
            return HookResult::Continue(text);
        }
        let data = serde_json::json!({
            "text": text,
            "images": images,
            "source": source,
            "streamingBehavior": streaming_behavior,
        });
        if let Ok(results) = self.runner.fire_event("input", data).await {
            if let Some(r) = results.as_array().and_then(|arr| arr.iter().find(|r| !r.is_null())) {
                if let Some(text) = r.as_str() {
                    return HookResult::Continue(text.to_string());
                }
                if let Some(text) = r.get("text").and_then(|v| v.as_str()) {
                    return HookResult::Continue(text.to_string());
                }
            }
        }
        HookResult::Continue(text)
    }

    async fn before_provider_request(&self, payload: Value) -> HookResult<Value> {
        if !self.has_handler("before_provider_request") {
            return HookResult::Continue(payload);
        }
        let data = serde_json::json!({ "payload": payload });
        if let Ok(results) = self.runner.fire_event("before_provider_request", data).await {
            if let Some(r) = results.as_array().and_then(|arr| arr.iter().find(|r| !r.is_null())) {
                if let Some(p) = r.get("payload") {
                    return HookResult::Continue(p.clone());
                }
                if !r.is_null() {
                    return HookResult::Continue(r.clone());
                }
            }
        }
        HookResult::Continue(payload)
    }

    async fn before_provider_headers(
        &self,
        headers: std::collections::HashMap<String, String>,
    ) -> HookResult<std::collections::HashMap<String, String>> {
        if !self.has_handler("before_provider_headers") {
            return HookResult::Continue(headers);
        }
        let data = serde_json::json!({ "headers": headers });
        if let Ok(results) = self.runner.fire_event("before_provider_headers", data).await {
            if let Some(r) = results.as_array().and_then(|arr| arr.iter().find(|r| !r.is_null())) {
                if let Some(h) = r
                    .get("headers")
                    .and_then(|v| serde_json::from_value::<std::collections::HashMap<String, String>>(v.clone()).ok())
                {
                    return HookResult::Continue(h);
                }
            }
        }
        HookResult::Continue(headers)
    }

    async fn after_provider_response(
        &self,
        status: u16,
        headers: std::collections::HashMap<String, String>,
    ) -> HookResult<()> {
        if self.has_handler("after_provider_response") {
            let _ = self
                .runner
                .fire_event(
                    "after_provider_response",
                    serde_json::json!({ "status": status, "headers": headers }),
                )
                .await;
        }
        HookResult::Continue(())
    }

    async fn before_session_switch(
        &self,
        reason: String,
        target_session_file: Option<String>,
    ) -> HookResult<()> {
        if !self.has_handler("session_before_switch") {
            return HookResult::Continue(());
        }
        let data = serde_json::json!({ "reason": reason, "targetSessionFile": target_session_file });
        if let Ok(results) = self.runner.fire_event("session_before_switch", data).await {
            if let Some(r) = results.as_array().and_then(|arr| arr.iter().find(|r| !r.is_null())) {
                if r.get("block").and_then(|b| b.as_bool()).unwrap_or(false) {
                    return HookResult::Cancel(
                        r.get("reason")
                            .and_then(|x| x.as_str())
                            .unwrap_or("blocked by extension")
                            .to_string(),
                    );
                }
            }
        }
        HookResult::Continue(())
    }

    async fn before_session_fork(&self, entry_id: String, position: String) -> HookResult<()> {
        if !self.has_handler("session_before_fork") {
            return HookResult::Continue(());
        }
        let data = serde_json::json!({ "entryId": entry_id, "position": position });
        if let Ok(results) = self.runner.fire_event("session_before_fork", data).await {
            if let Some(r) = results.as_array().and_then(|arr| arr.iter().find(|r| !r.is_null())) {
                if r.get("block").and_then(|b| b.as_bool()).unwrap_or(false) {
                    return HookResult::Cancel(
                        r.get("reason")
                            .and_then(|x| x.as_str())
                            .unwrap_or("blocked by extension")
                            .to_string(),
                    );
                }
            }
        }
        HookResult::Continue(())
    }

    async fn before_session_compact(&self, reason: String, will_retry: bool) -> HookResult<()> {
        if !self.has_handler("session_before_compact") {
            return HookResult::Continue(());
        }
        let data = serde_json::json!({ "reason": reason, "willRetry": will_retry });
        if let Ok(results) = self.runner.fire_event("session_before_compact", data).await {
            if let Some(r) = results.as_array().and_then(|arr| arr.iter().find(|r| !r.is_null())) {
                if r.get("block").and_then(|b| b.as_bool()).unwrap_or(false) {
                    return HookResult::Cancel(
                        r.get("reason")
                            .and_then(|x| x.as_str())
                            .unwrap_or("blocked by extension")
                            .to_string(),
                    );
                }
            }
        }
        HookResult::Continue(())
    }

    async fn before_session_tree(&self, target_id: String) -> HookResult<()> {
        if !self.has_handler("session_before_tree") {
            return HookResult::Continue(());
        }
        let data = serde_json::json!({ "targetId": target_id });
        if let Ok(results) = self.runner.fire_event("session_before_tree", data).await {
            if let Some(r) = results.as_array().and_then(|arr| arr.iter().find(|r| !r.is_null())) {
                if r.get("block").and_then(|b| b.as_bool()).unwrap_or(false) {
                    return HookResult::Cancel(
                        r.get("reason")
                            .and_then(|x| x.as_str())
                            .unwrap_or("blocked by extension")
                            .to_string(),
                    );
                }
            }
        }
        HookResult::Continue(())
    }
}

// ============================================================================
// load_bun_extensions — 顶层集成：discover + spawn + adapt
// ============================================================================

use super::loader::discover_extension_paths;

/// 加载结果：adapters + runner（runner 必须存活到会话结束）。
pub struct BunExtensionsLoaded {
    pub adapters: Vec<BunExtensionAdapter>,
    pub runner: Arc<BunExtensionRunner>,
}

/// 发现并加载 JS/TS 扩展（Bun 子进程）。
///
/// 发现 → 每个扩展 spawn 一个
/// Bun 子进程 → 收集注册 → 产出 adapter。多个扩展各自独立子进程。
///
/// # Errors
/// 返回每个扩展的加载错误（失败的跳过，成功的仍返回）。
pub async fn load_bun_extensions(
    extension_paths: &[String],
    cwd: &str,
    agent_dir: &str,
    flags: &std::collections::HashMap<String, String>,
) -> Result<Option<BunExtensionsLoaded>, Vec<String>> {
    let discovered = discover_extension_paths(extension_paths, cwd, agent_dir);
    if discovered.paths.is_empty() {
        return Ok(None);
    }

    let mut adapters = Vec::new();
    let mut errors = Vec::new();
    let mut runner: Option<Arc<BunExtensionRunner>> = None;

    for ext_path in &discovered.paths {
        let path_str = ext_path.to_string_lossy().to_string();
        match BunExtensionRunner::spawn(ext_path, Path::new(cwd), Path::new(agent_dir), flags).await {
            Ok(r) => {
                let load_result = r.take_load_result().await;
                for log in &load_result.logs {
                    eprintln!("[pi] extension: {log}");
                }
                if load_result.tools.is_empty()
                    && load_result.commands.is_empty()
                    && load_result.shortcuts.is_empty()
                    && load_result.flags.is_empty()
                    && load_result.handlers.is_empty()
                    && load_result.pending_providers.is_empty()
                {
                    r.shutdown().await;
                    continue;
                }
                let r = Arc::new(r);
                let adapter = BunExtensionAdapter::new(&path_str, load_result, r.clone());
                adapters.push(adapter);
                runner = Some(r);
            }
            Err(e) => {
                errors.push(format!("Failed to load extension {path_str}: {e}"));
            }
        }
    }

    if adapters.is_empty() {
        // 全部失败：把错误返回给调用方（V8 路径同款行为）。
        if errors.is_empty() {
            return Ok(None);
        }
        return Err(errors);
    }

    Ok(Some(BunExtensionsLoaded {
        adapters,
        runner: runner.expect("at least one adapter implies a runner"),
    }))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// 写一个临时扩展文件，返回其路径。
    fn write_ext(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    /// 端到端：Bun 子进程加载扩展 → 注册工具 → 执行工具。
    #[tokio::test]
    async fn test_bun_load_and_execute_tool() {
        let dir = tempfile::tempdir().unwrap();
        let ext = write_ext(
            dir.path(),
            "echo.ts",
            r#"
export default function(pi) {
  pi.registerTool({
    name: "echo",
    description: "echo back",
    execute: async (callId, params) => {
      return { content: [{ type: "text", text: "echo: " + (params.n ?? 0) }], details: { doubled: (params.n ?? 0) * 2 } };
    },
  });
  pi.on("agent_start", () => {});
}
"#,
        );
        let agent_dir = tempfile::tempdir().unwrap();
        let paths = vec![ext.to_string_lossy().to_string()];
        let flags: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let loaded = load_bun_extensions(
            &paths,
            "/tmp",
            &agent_dir.path().to_string_lossy(),
            &flags,
        )
        .await
        .expect("load")
        .expect("some extensions");
        assert_eq!(loaded.adapters.len(), 1);
        let adapter = &loaded.adapters[0];
        assert_eq!(adapter.name(), "echo");

        let ctx = ExtensionContext::new(
            "/tmp".to_string(),
            false,
            pi_extension_api::ExtensionUIContext {
                notify: std::sync::Arc::new(|_, _| {}),
                set_status: std::sync::Arc::new(|_, _| {}),
                confirm: std::sync::Arc::new(|_, _| false),
            },
            pi_extension_api::RuntimeHandle::noop(),
        );
        let output = adapter
            .handle_tool_call("echo", serde_json::json!({ "n": 21 }), &ctx)
            .await
            .expect("tool call");
        assert!(!output.is_error);
        assert_eq!(output.content[0]["text"], "echo: 21");
        let details = output.details.expect("details");
        assert_eq!(details["doubled"], 42);
    }

    /// 扩展注册了 handler 时，事件能通过 RPC 触发到 Bun 侧。
    #[tokio::test]
    async fn test_bun_fire_event() {
        let dir = tempfile::tempdir().unwrap();
        let ext = write_ext(
            dir.path(),
            "events.ts",
            r#"
export default function(pi) {
  pi.on("agent_start", (data, ctx) => {
    pi.log("agent_start fired, cwd=" + ctx.cwd);
  });
}
"#,
        );
        let agent_dir = tempfile::tempdir().unwrap();
        let paths = vec![ext.to_string_lossy().to_string()];
        let flags: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let loaded = load_bun_extensions(
            &paths,
            "/tmp",
            &agent_dir.path().to_string_lossy(),
            &flags,
        )
        .await
        .expect("load")
        .expect("some extensions");
        let adapter = &loaded.adapters[0];
        adapter.on_agent_start().await;
        // 事件已触发（无 panic 即通过；日志经 stderr 输出）。
    }
        /// Action 方法经 RPC 往返：扩展工具内调用 `pi.getCommands()` / `pi.exec()`，
        /// 宿主 bind_actions 后返回正确结果。回归：早期协议 id 冲突导致返回 `[]`。
        #[tokio::test]
        async fn test_bun_action_methods_via_rpc() {
            let dir = tempfile::tempdir().unwrap();
            let ext = write_ext(
                dir.path(),
                "actions.ts",
                r#"
export default function(pi) {
  pi.registerTool({
    name: "act",
    description: "test actions",
    execute: async () => {
      const cmds = await pi.getCommands();
      const r = await pi.exec("echo", ["hi"]);
      return { content: [{ type: "text", text: "cmds=" + JSON.stringify(cmds) + " exec=" + JSON.stringify(r) }] };
    },
  });
}
"#,
            );
            let agent_dir = tempfile::tempdir().unwrap();
            let paths = vec![ext.to_string_lossy().to_string()];
            let flags: std::collections::HashMap<String, String> = std::collections::HashMap::new();
            let loaded = load_bun_extensions(
                &paths,
                "/tmp",
                &agent_dir.path().to_string_lossy(),
                &flags,
            )
            .await
            .expect("load")
            .expect("some");
            // 模拟 sdk.rs 的 bind_actions
            let actions = BunRuntimeActions {
                get_commands: Some(std::sync::Arc::new(|| {
                    serde_json::json!([{ "name": "goal", "source": "extension" }])
                        .as_array()
                        .unwrap()
                        .clone()
                })),
                ..Default::default()
            };
            loaded.runner.bind_actions(actions).await;

            let adapter = &loaded.adapters[0];
            let ctx = ExtensionContext::new(
                "/tmp".to_string(),
                false,
                pi_extension_api::ExtensionUIContext {
                    notify: std::sync::Arc::new(|_, _| {}),
                    set_status: std::sync::Arc::new(|_, _| {}),
                    confirm: std::sync::Arc::new(|_, _| false),
                },
                pi_extension_api::RuntimeHandle::noop(),
            );
            let output = adapter
                .handle_tool_call("act", serde_json::json!({}), &ctx)
                .await
                .expect("tool call");
            let text = output.content[0]["text"].as_str().unwrap_or("");
            assert!(
                text.contains(r#""name":"goal""#),
                "getCommands result missing, got: {text}"
            );
            assert!(text.contains("\"stdout\":\"hi\\n\""), "exec result missing, got: {text}");
        }

        /// 工具工厂桥接：createBashTool 的 execute 经 RPC 到宿主跑真实内置工具。
        #[tokio::test]
        async fn test_bun_tool_factory_bridge() {
            let dir = tempfile::tempdir().unwrap();
            let ext = write_ext(
                dir.path(),
                "factory.ts",
                r#"
import { createBashTool } from "@earendil-works/pi-coding-agent";
export default function(pi) { pi.registerTool(createBashTool()); }
"#,
            );
            let agent_dir = tempfile::tempdir().unwrap();
            let paths = vec![ext.to_string_lossy().to_string()];
            let flags: std::collections::HashMap<String, String> = std::collections::HashMap::new();
            let loaded = load_bun_extensions(
                &paths,
                "/tmp",
                &agent_dir.path().to_string_lossy(),
                &flags,
            )
            .await
            .expect("load")
            .expect("some");
            assert_eq!(loaded.adapters.len(), 1);
            // 模拟 sdk.rs 的 bind_actions：run_builtin_tool 跑宿主内置工具。
            let actions = BunRuntimeActions {
                run_builtin_tool: Some(std::sync::Arc::new({
                    let tools = crate::core::tools::create_coding_tools("/tmp", None);
                    move |name: String, params: serde_json::Value| {
                        let tools = tools.clone();
                        Box::pin(async move {
                            let tool = tools
                                .iter()
                                .find(|t| t.name == name)
                                .ok_or_else(|| format!("not found: {name}"))?;
                            let result = (tool.execute)("c1".to_string(), params, None, None)
                                .await
                                .map_err(|e| e.to_string())?;
                            serde_json::to_value(result).map_err(|e| e.to_string())
                        })
                    }
                })),
                ..Default::default()
            };
            loaded.runner.bind_actions(actions).await;

            let adapter = &loaded.adapters[0];
            let ctx = ExtensionContext::new(
                "/tmp".to_string(),
                false,
                pi_extension_api::ExtensionUIContext {
                    notify: std::sync::Arc::new(|_, _| {}),
                    set_status: std::sync::Arc::new(|_, _| {}),
                    confirm: std::sync::Arc::new(|_, _| false),
                },
                pi_extension_api::RuntimeHandle::noop(),
            );
            let output = adapter
                .handle_tool_call("bash", serde_json::json!({ "command": "echo hi" }), &ctx)
                .await
                .expect("tool call");
            assert!(!output.is_error, "bash tool should succeed");
            let text = output.content[0]["text"].as_str().unwrap_or("");
            assert!(text.contains("hi"), "got: {text}");
        }
    }
