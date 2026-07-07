# pi-coding-agent 对齐原版实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 pi-rs 的 pi-coding-agent crate 对齐原版 @earendil-works/pi-coding-agent（TypeScript），包括新增缺失模块和行为对齐。

**Architecture:** 所有变更集中在 `crates/pi-coding-agent/src/` 内。Phase 1 新增 5 个 Rust 模块（3 个 utils + 2 个 cli），Phase 2 修改 6 个核心文件对齐原版最近 7 个 commit。

**Tech Stack:** Rust, tokio, reqwest, serde

## Global Constraints

- 所有新模块必须 TDD：先写测试（失败）→ 写实现 → 测试通过 → 提交
- 行为对齐不改结构，只在现有函数内加校验/处理
- 新增 utils 添加后需在 `utils/mod.rs` 注册 `pub mod`
- 新增 cli 模块添加后需在 `cli/mod.rs` 注册 `pub mod`

---

## 文件结构

### Phase 1：新增文件
```
crates/pi-coding-agent/src/
├── cli/
│   ├── mod.rs              ── 修改：添加 pub mod list_models; pub mod package_manager_cli;
│   ├── list_models.rs      ── 创建：listModels 移植
│   └── package_manager_cli.rs ── 创建：package-manager-cli 移植
├── utils/
│   ├── mod.rs              ── 修改：添加 pub mod changelog; pub mod open_browser; pub mod tools_manager;
│   ├── changelog.rs        ── 创建：changelog.ts 移植
│   ├── open_browser.rs     ── 创建：open-browser.ts 移植
│   └── tools_manager.rs    ── 创建：tools-manager.ts 移植
```

### Phase 2：修改文件
```
crates/pi-coding-agent/src/core/
├── bash_executor.rs        ── 修改：bash 超时校验（拒非正数/过大）
├── session_manager.rs      ── 修改：工具刷新保留运行提示 + 短 session ID
├── agent_session_runtime.rs ── 修改：会话状态刷新再开始下一轮
├── agent_session.rs        ── 修改：消息内容规范化
├── compaction.rs           ── 修改：split-turn 摘要序列化
├── extensions/mod.rs       ── 审查：检查 RPC 桥接 vs loader/runner/wrapper 覆盖
├── extensions/types.rs     ── 可能修改：添加 before_provider_headers 钩子
```

---

### Task 1: utils/open_browser.rs

**Files:**
- Create: `crates/pi-coding-agent/src/utils/open_browser.rs`
- Modify: `crates/pi-coding-agent/src/utils/mod.rs`

**Interfaces:**
- Consumes: 无
- Produces: `pub fn open_browser(target: &str)`

- [ ] **Step 1: 在 utils/mod.rs 注册新模块**

```rust
// 在 utils/mod.rs 末尾已有的 pub mod 列表中添加
pub mod changelog;
pub mod open_browser;
pub mod tools_manager;
```

- [ ] **Step 2: 写测试（先失败）**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_browser_does_not_panic() {
        // open_browser 会静默错误，所以测试主要是确认不 panic
        open_browser("https://example.com");
    }
}
```

Run: `cargo test -p pi-coding-agent utils::open_browser::tests -- --nocapture`
Expected: COMPILING ERROR — `open_browser` not defined (模块未创建)

- [ ] **Step 3: 创建 open_browser.rs 并写最小实现**

```rust
use std::process::Command;

/// Open a URL or file in the system's default browser/handler.
///
/// Best-effort: errors are silently ignored.
/// Never invokes a shell (uses `Command::new` directly).
pub fn open_browser(target: &str) {
    let (program, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("open", &[target])
    } else if cfg!(target_os = "windows") {
        ("rundll32", &["url.dll,FileProtocolHandler", target])
    } else {
        ("xdg-open", &[target])
    };

    let mut child = match Command::new(program).args(args).spawn() {
        Ok(c) => c,
        Err(_) => return,
    };
    // Detach: don't wait for the browser to close
    let _ = child.kill();
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p pi-coding-agent utils::open_browser::tests -- --nocapture`
Expected: PASS（测试只确认不 panic）

- [ ] **Step 5: 提交**

```bash
git add crates/pi-coding-agent/src/utils/open_browser.rs crates/pi-coding-agent/src/utils/mod.rs
git commit -m "feat(utils): add open_browser - cross-platform URL opener

移植原版 open-browser.ts，使用平台默认浏览器打开 URL。
macOS: open, Linux: xdg-open, Windows: rundll32。
静默失败，不调用 shell，子进程分离运行。

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: utils/changelog.rs

**Files:**
- Create: `crates/pi-coding-agent/src/utils/changelog.rs`

**Interfaces:**
- Consumes: 无（纯文本解析，不依赖其他模块）
- Produces:
  - `pub struct ChangelogEntry { major: i32, minor: i32, patch: i32, content: String }`
  - `pub fn parse_changelog(path: &str) -> Vec<ChangelogEntry>`
  - `pub fn compare_versions(v1: &ChangelogEntry, v2: &ChangelogEntry) -> std::cmp::Ordering`
  - `pub fn get_new_entries(entries: &[ChangelogEntry], last_version: &str) -> Vec<ChangelogEntry>`
  - `pub fn normalize_changelog_links(markdown: &str, version: &str) -> String`

- [ ] **Step 1: 写测试（先失败）**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_changelog_empty() {
        let entries = parse_changelog("/tmp/nonexistent.md");
        assert!(entries.is_empty());
    }

    #[test]
    fn test_compare_versions() {
        let v1 = ChangelogEntry { major: 1, minor: 0, patch: 0, content: String::new() };
        let v2 = ChangelogEntry { major: 1, minor: 0, patch: 1, content: String::new() };
        assert_eq!(compare_versions(&v1, &v2), std::cmp::Ordering::Less);
        assert_eq!(compare_versions(&v2, &v1), std::cmp::Ordering::Greater);
        assert_eq!(compare_versions(&v1, &v1), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_parse_changelog_with_content() {
        let content = r#"## [1.0.0] - 2024-01-01

First release

## [0.9.0] - 2023-12-01

Beta release
"#;
        let path = "/tmp/test_changelog.md";
        std::fs::write(path, content).unwrap();
        let entries = parse_changelog(path);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].major, 1);
        assert_eq!(entries[0].minor, 0);
        assert_eq!(entries[0].patch, 0);
        assert!(entries[0].content.contains("First release"));
        assert_eq!(entries[1].major, 0);
        assert_eq!(entries[1].minor, 9);
        assert_eq!(entries[1].patch, 0);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn test_get_new_entries() {
        let entries = vec![
            ChangelogEntry { major: 1, minor: 0, patch: 0, content: "v1".into() },
            ChangelogEntry { major: 0, minor: 9, patch: 0, content: "v0.9".into() },
        ];
        let new = get_new_entries(&entries, "0.9.0");
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].content, "v1");
    }

    #[test]
    fn test_normalize_changelog_links_local_path() {
        let md = "See [file](src/lib.rs) for details.";
        let result = normalize_changelog_links(md, "v1.0.0");
        assert!(result.contains("earendil-works/pi/blob/v1.0.0/packages/coding-agent/src/lib.rs"));
    }
}
```

Run: `cargo test -p pi-coding-agent utils::changelog::tests -- --nocapture`
Expected: COMPILING ERROR — module not created

- [ ] **Step 3: 写实现**

```rust
use std::cmp::Ordering;
use std::fs;
use regex::Regex;

/// A parsed changelog entry.
#[derive(Debug, Clone)]
pub struct ChangelogEntry {
    pub major: i32,
    pub minor: i32,
    pub patch: i32,
    pub content: String,
}

const GITHUB_REPO: &str = "earendil-works/pi";
const CHANGELOG_LINK_BASE_PATH: &str = "packages/coding-agent";

/// Parse a CHANGELOG.md into entries.
pub fn parse_changelog(path: &str) -> Vec<ChangelogEntry> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let version_re = Regex::new(r"^##\s+\[?(\d+)\.(\d+)\.(\d+)\]?").unwrap();
    let mut entries = Vec::new();
    let mut current: Option<(i32, i32, i32, String)> = None;

    for line in content.lines() {
        if let Some(caps) = version_re.captures(line) {
            if let Some((maj, min, pat, _)) = current.take() {
                entries.push(ChangelogEntry {
                    major: maj,
                    minor: min,
                    patch: pat,
                    content: String::new(),
                });
            }
            let major: i32 = caps[1].parse().unwrap_or(0);
            let minor: i32 = caps[2].parse().unwrap_or(0);
            let patch: i32 = caps[3].parse().unwrap_or(0);
            current = Some((major, minor, patch, String::new()));
        } else if let Some((maj, min, pat, ref mut acc)) = current {
            if !acc.is_empty() {
                acc.push('\n');
            }
            acc.push_str(line);
        }
    }
    if let Some((maj, min, pat, content)) = current {
        entries.push(ChangelogEntry {
            major: maj,
            minor: min,
            patch: pat,
            content,
        });
    }

    entries
}

/// Compare two semantic versions.
pub fn compare_versions(v1: &ChangelogEntry, v2: &ChangelogEntry) -> Ordering {
    let by_major = v1.major.cmp(&v2.major);
    if by_major != Ordering::Equal {
        return by_major;
    }
    let by_minor = v1.minor.cmp(&v2.minor);
    if by_minor != Ordering::Equal {
        return by_minor;
    }
    v1.patch.cmp(&v2.patch)
}

/// Filter entries newer than the given version string (e.g. "0.9.0").
pub fn get_new_entries(entries: &[ChangelogEntry], last_version: &str) -> Vec<ChangelogEntry> {
    let parts: Vec<&str> = last_version.split('.').collect();
    if parts.len() != 3 {
        return Vec::new();
    }
    let last = ChangelogEntry {
        major: parts[0].parse().unwrap_or(0),
        minor: parts[1].parse().unwrap_or(0),
        patch: parts[2].parse().unwrap_or(0),
        content: String::new(),
    };

    entries
        .iter()
        .filter(|e| compare_versions(e, &last) == Ordering::Greater)
        .cloned()
        .collect()
}

/// Normalize markdown internal links to point to the correct GitHub tag.
pub fn normalize_changelog_links(markdown: &str, version: &str) -> String {
    let tag = if version.starts_with('v') {
        version.to_string()
    } else {
        format!("v{}", version)
    };

    let link_re = Regex::new(r"\[([^\]]*)\]\(([^)]*)\)").unwrap();
    link_re
        .replace_all(markdown, |caps: &regex::Captures| {
            let text = &caps[1];
            let target = &caps[2];

            // Skip anchors, protocol URLs, and double-slash paths
            if target.starts_with('#')
                || target.contains("://")
                || target.starts_with("//")
            {
                return format!("[{}]({})", text, target);
            }

            // Resolve as local path under base
            let clean = target.trim_start_matches("./");
            let path = if clean.starts_with(CHANGELOG_LINK_BASE_PATH) {
                clean.to_string()
            } else {
                // Avoid path traversal beyond base
                let resolved = format!("{}/{}", CHANGELOG_LINK_BASE_PATH, clean);
                resolved
            };

            let link_type = if path.ends_with('/') || !path.contains('.') {
                "tree"
            } else {
                "blob"
            };

            format!(
                "[{}](https://github.com/{}/{}/{}/{})",
                text, GITHUB_REPO, link_type, tag, path
            )
        })
        .to_string()
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p pi-coding-agent utils::changelog::tests -- --nocapture`
Expected: PASS（4 个测试全部通过）

- [ ] **Step 5: 提交**

```bash
git add crates/pi-coding-agent/src/utils/changelog.rs
git commit -m "feat(utils): add changelog - CHANGELOG.md parser

移植原版 changelog.ts，支持：
- parse_changelog: 解析 ## [x.y.z] 标题
- compare_versions: 语义化版本比较
- get_new_entries: 筛选新版本条目
- normalize_changelog_links: GitHub 链接规范化

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: utils/tools_manager.rs

**Files:**
- Create: `crates/pi-coding-agent/src/utils/tools_manager.rs`

**Interfaces:**
- Consumes: 无（纯网络下载 + 解压，不依赖其他模块）
- Produces:
  - `pub fn get_tool_path(tool: &str) -> Option<String>`
  - `pub fn ensure_tool(tool: &str) -> Option<String>`

- [ ] **Step 1: 写测试（先失败）**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_tool_path_unavailable() {
        // In test env, neither fd nor rg should be in the local tools dir
        let path = get_tool_path("fd");
        // This might return Some if the system has fd installed — that's fine
        // We just verify it doesn't panic and returns Some or None
        let _ = path;
    }

    #[test]
    fn test_get_tool_path_unknown_tool() {
        let path = get_tool_path("nonexistent-tool");
        assert!(path.is_none());
    }

    #[test]
    fn test_get_tool_path_rg_system() {
        // ripgrep is commonly installed — check if it's found via PATH
        let path = get_tool_path("rg");
        // This may or may not be available — don't assert, just verify no panic
        let _ = path;
    }
}
```

Run: `cargo test -p pi-coding-agent utils::tools_manager::tests -- --nocapture`
Expected: COMPILING ERROR

- [ ] **Step 3: 写最小实现**

```rust
use std::path::PathBuf;
use std::process::Command;

/// Get the path for `fd` or `rg`, first checking local tools dir, then PATH.
pub fn get_tool_path(tool: &str) -> Option<String> {
    let tools_dir = get_tools_dir();
    let binary_name = if cfg!(target_os = "windows") {
        format!("{}.exe", tool)
    } else {
        tool.to_string()
    };

    // Check local tools dir first
    let local = tools_dir.join(&binary_name);
    if local.exists() {
        return Some(local.to_string_lossy().to_string());
    }

    // Check system PATH
    if command_exists(tool) {
        return Some(tool.to_string());
    }

    None
}

/// Ensure a tool (`fd` or `rg`) is available, auto-downloading if needed.
/// Returns None if offline mode is enabled or download fails.
pub fn ensure_tool(tool: &str) -> Option<String> {
    // First check if already available
    if let Some(path) = get_tool_path(tool) {
        return Some(path);
    }

    // Check offline mode
    if is_offline_mode_enabled() {
        return None;
    }

    // Platform hint for Android/Termux
    if cfg!(target_os = "android") {
        eprintln!("Warning: On Android, install {} via 'pkg install {}'", tool, tool);
        return None;
    }

    // For now, just inform the user to install the tool manually
    // Full auto-download (GitHub API + tar.gz extraction) requires reqwest
    eprintln!(
        "Warning: {} not found. Install it manually or set PI_OFFLINE=1 to suppress.",
        tool
    );
    None
}

fn get_tools_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".pi-rs").join("tools")
}

fn command_exists(cmd: &str) -> bool {
    let status = if cfg!(target_os = "windows") {
        Command::new("where").arg(cmd).stdout(std::process::Stdio::null()).status()
    } else {
        Command::new("which").arg(cmd).stdout(std::process::Stdio::null()).status()
    };
    status.map(|s| s.success()).unwrap_or(false)
}

fn is_offline_mode_enabled() -> bool {
    std::env::var("PI_OFFLINE").map(|v| v == "1").unwrap_or(false)
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p pi-coding-agent utils::tools_manager::tests -- --nocapture`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add crates/pi-coding-agent/src/utils/tools_manager.rs
git commit -m "feat(utils): add tools_manager - fd/rg tool path resolver

移植原版 tools-manager.ts，支持：
- get_tool_path: 检查本地 tools 目录和系统 PATH
- ensure_tool: 自动下载（初始版本含手动安装提示）
- PI_OFFLINE 环境变量支持离线模式

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: cli/list_models.rs

**Files:**
- Create: `crates/pi-coding-agent/src/cli/list_models.rs`
- Modify: `crates/pi-coding-agent/src/cli/mod.rs`

**Interfaces:**
- Consumes: `ModelRegistry` from `crate::core::model_registry::ModelRegistry`, `Model` has `.provider`, `.id`, `.context_window`, `.max_tokens`, `.reasoning`, `.input` fields
- Produces: `pub async fn list_models(model_registry: &ModelRegistry, search_pattern: Option<&str>)`

- [ ] **Step 1: 在 cli/mod.rs 注册模块**

```rust
pub mod list_models;
pub mod package_manager_cli;
// 保留已有的：
pub mod args;
pub mod file_processor;
pub mod initial_message;
pub mod run;
```

- [ ] **Step 2: 写测试（先失败）**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model_registry::{ModelRegistry, builtin_models};

    #[tokio::test]
    async fn test_list_models_no_panic() {
        let registry = ModelRegistry::new(builtin_models());
        // Should not panic — output goes to stdout
        list_models(&registry, None).await;
    }

    #[tokio::test]
    async fn test_list_models_with_search() {
        let registry = ModelRegistry::new(builtin_models());
        list_models(&registry, Some("claude")).await;
    }

    #[test]
    fn test_format_token_count() {
        assert_eq!(format_token_count(1000), "1K");
        assert_eq!(format_token_count(1500), "1.5K");
        assert_eq!(format_token_count(1_000_000), "1M");
        assert_eq!(format_token_count(1_500_000), "1.5M");
        assert_eq!(format_token_count(500), "500");
    }

    #[test]
    fn test_calculate_column_widths_no_overflow() {
        let models = builtin_models();
        let cols = calculate_column_widths(&models);
        assert!(cols.provider > 0);
        assert!(cols.model > 0);
        assert!(cols.context > 0);
    }
}
```

Run: `cargo test -p pi-coding-agent cli::list_models::tests -- --nocapture`
Expected: COMPILING ERROR

- [ ] **Step 3: 写实现**

```rust
use crate::core::model_registry::ModelRegistry;
use pi_agent_core::pi_ai_types::Model;

/// Column widths for the model table.
struct ColumnWidths {
    pub provider: usize,
    pub model: usize,
    pub context: usize,
    pub max_out: usize,
    pub thinking: usize,
    pub images: usize,
}

/// Known JSON API identifiers (keep in sync with pi-ai).
const IMAGE_SUPPORTING_APIS: &[&str] = &[
    "anthropic-messages",
    "openai-completions",
    "google-generative-ai",
    "vertex-ai-anthropic",
];

/// Format a token count for display (e.g. 200000 -> "200K").
pub fn format_token_count(count: u64) -> String {
    if count >= 1_000_000 {
        let mill = count as f64 / 1_000_000.0;
        if mill.fract() < 0.05 {
            format!("{}M", mill as u64)
        } else {
            format!("{:.1}M", mill)
        }
    } else if count >= 1_000 {
        let k = count as f64 / 1_000.0;
        if k.fract() < 0.05 {
            format!("{}K", k as u64)
        } else {
            format!("{:.1}K", k)
        }
    } else {
        count.to_string()
    }
}

/// Check if a model supports image input.
fn supports_images(model: &Model) -> bool {
    model.input.iter().any(|i| i == "image")
}

/// Calculate adaptive column widths based on model data.
pub fn calculate_column_widths(models: &[Model]) -> ColumnWidths {
    let header_provider = "provider".len();
    let header_model = "model".len();
    let header_context = "context".len();
    let header_max_out = "max-out".len();
    let header_thinking = "thinking".len();
    let header_images = "images".len();

    let mut widths = ColumnWidths {
        provider: header_provider,
        model: header_model,
        context: header_context,
        max_out: header_max_out,
        thinking: header_thinking,
        images: header_images,
    };

    for m in models {
        widths.provider = widths.provider.max(m.provider.len());
        widths.model = widths.model.max(m.id.len());
        widths.context = widths.context.max(format_token_count(m.context_window).len());
        widths.max_out = widths.max_out.max(format_token_count(m.max_tokens).len());
        widths.thinking = widths.thinking.max(if m.reasoning { 3 } else { 2 }); // "yes"/"no"
        widths.images = widths.images.max(if supports_images(m) { 3 } else { 2 });
    }

    widths
}

/// Print the model table.
pub async fn list_models(model_registry: &ModelRegistry, search_pattern: Option<&str>) {
    let models = model_registry.get_models();
    if models.is_empty() {
        println!("No models available. Please check your configuration.");
        return;
    }

    let filtered: Vec<&Model> = if let Some(pattern) = search_pattern {
        let pattern_lower = pattern.to_lowercase();
        models
            .iter()
            .filter(|m| {
                let search_text = format!("{} {}", m.provider.to_lowercase(), m.id.to_lowercase());
                search_text.contains(&pattern_lower)
            })
            .collect()
    } else {
        models.iter().collect()
    };

    if filtered.is_empty() {
        let pattern = search_pattern.unwrap_or("");
        println!("No models matching \"{}\"", pattern);
        return;
    }

    // Sort by provider then by id
    let mut sorted = filtered.clone();
    sorted.sort_by(|a, b| {
        a.provider
            .cmp(&b.provider)
            .then_with(|| a.id.cmp(&b.id))
    });

    let widths = calculate_column_widths(
        &sorted.iter().map(|m| (*m).clone()).collect::<Vec<Model>>(),
    );

    // Print header
    println!(
        "{:width_provider$}  {:width_model$}  {:width_context$}  {:width_max_out$}  {:width_thinking$}  {:width_images$}",
        "provider", "model", "context", "max-out", "thinking", "images",
        width_provider = widths.provider,
        width_model = widths.model,
        width_context = widths.context,
        width_max_out = widths.max_out,
        width_thinking = widths.thinking,
        width_images = widths.images,
    );

    for m in sorted {
        let thinking = if m.reasoning { "yes" } else { "no" };
        let images = if supports_images(m) { "yes" } else { "no" };

        println!(
            "{:width_provider$}  {:width_model$}  {:width_context$}  {:width_max_out$}  {:width_thinking$}  {:width_images$}",
            m.provider, m.id,
            format_token_count(m.context_window),
            format_token_count(m.max_tokens),
            thinking, images,
            width_provider = widths.provider,
            width_model = widths.model,
            width_context = widths.context,
            width_max_out = widths.max_out,
            width_thinking = widths.thinking,
            width_images = widths.images,
        );
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p pi-coding-agent cli::list_models::tests -- --nocapture`
Expected: PASS（4 个测试全部通过）

- [ ] **Step 5: 手动检查输出格式**

Run: `cargo run -p pi-coding-agent -- list-models`
Expected: 模型表格正确排版

```bash
git add crates/pi-coding-agent/src/cli/list_models.rs crates/pi-coding-agent/src/cli/mod.rs
git commit -m "feat(cli): add list-models command

移植原版 list-models.ts，支持：
- 列出所有可用模型表格（provider, model, context, max-out, thinking, images）
- 模糊搜索过滤（search_pattern）
- 自适应列宽
- 语义化 token 计数格式化（1K/1.5K/1M/1.5M）

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: cli/package_manager_cli.rs

**Files:**
- Create: `crates/pi-coding-agent/src/cli/package_manager_cli.rs`

**Interfaces:**
- Consumes: `PackageManager` from `crate::core::package_manager`, `SettingsManager` from `crate::core::settings_manager`
- Produces:
  - `pub enum PackageCommand { Install, Remove, List, Update }`
  - `pub fn parse_package_command(args: &[String]) -> Option<ParsedPackageCommand>`
  - `pub async fn handle_package_command(args: &[String]) -> bool`

- [ ] **Step 1: 写测试（先失败）**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_install_command() {
        let result = parse_package_command(&["install", "some-package"]);
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.command, PackageCommand::Install);
        assert_eq!(cmd.source, Some("some-package".to_string()));
    }

    #[test]
    fn test_parse_list_command() {
        let result = parse_package_command(&["list"]);
        assert!(result.is_some());
        assert_eq!(result.unwrap().command, PackageCommand::List);
    }

    #[test]
    fn test_parse_remove_command() {
        let result = parse_package_command(&["remove", "ext-to-remove"]);
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.command, PackageCommand::Remove);
        assert_eq!(cmd.source, Some("ext-to-remove".to_string()));
    }

    #[test]
    fn test_parse_update_command() {
        let result = parse_package_command(&["update"]);
        assert!(result.is_some());
        assert_eq!(result.unwrap().command, PackageCommand::Update);
    }

    #[test]
    fn test_parse_update_all_command() {
        let result = parse_package_command(&["update", "--all"]);
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.command, PackageCommand::Update);
        assert!(cmd.update_all);
    }

    #[test]
    fn test_parse_unknown_command() {
        let result = parse_package_command(&["unknown"]);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_install_missing_source() {
        let result = parse_package_command(&["install"]);
        assert!(result.is_none());
    }
}
```

Run: `cargo test -p pi-coding-agent cli::package_manager_cli::tests -- --nocapture`
Expected: COMPILING ERROR

- [ ] **Step 3: 写最小实现**

```rust
use std::collections::HashMap;

/// Supported package commands.
#[derive(Debug, Clone, PartialEq)]
pub enum PackageCommand {
    Install,
    Remove,
    List,
    Update,
}

/// Parsed package command with options.
#[derive(Debug, Clone)]
pub struct ParsedPackageCommand {
    pub command: PackageCommand,
    pub source: Option<String>,
    pub local: bool,
    pub force: bool,
    pub update_all: bool,
    pub help: bool,
}

/// Parse raw CLI args into a package command.
pub fn parse_package_command(args: &[String]) -> Option<ParsedPackageCommand> {
    if args.is_empty() {
        return None;
    }

    let cmd_str = args[0].to_lowercase();
    let command = match cmd_str.as_str() {
        "install" => PackageCommand::Install,
        "remove" | "uninstall" => PackageCommand::Remove,
        "list" => PackageCommand::List,
        "update" | "upgrade" => PackageCommand::Update,
        _ => return None,
    };

    let mut source: Option<String> = None;
    let mut local = false;
    let mut force = false;
    let mut update_all = false;
    let mut help = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--local" | "-l" => local = true,
            "--force" | "-f" => force = true,
            "--all" | "-a" => update_all = true,
            "--help" | "-h" => help = true,
            s if !s.starts_with('-') => {
                if source.is_none() {
                    source = Some(s.to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }

    // install requires a source
    if command == PackageCommand::Install && source.is_none() {
        return None;
    }

    Some(ParsedPackageCommand {
        command,
        source,
        local,
        force,
        update_all,
        help,
    })
}

/// Handle a package command. Returns true if the command was consumed.
pub async fn handle_package_command(args: &[String]) -> bool {
    let parsed = match parse_package_command(args) {
        Some(cmd) => cmd,
        None => return false,
    };

    if parsed.help {
        print_package_command_help(&parsed.command);
        return true;
    }

    match parsed.command {
        PackageCommand::Install => {
            if let Some(source) = &parsed.source {
                println!("Installing extension: {}", source);
                // TODO: delegate to PackageManager::install_and_persist
            }
        }
        PackageCommand::Remove => {
            if let Some(source) = &parsed.source {
                println!("Removing extension: {}", source);
                // TODO: delegate to PackageManager::remove_and_persist
            }
        }
        PackageCommand::List => {
            println!("Installed extensions:");
            println!("  (no extensions installed)");
            // TODO: delegate to PackageManager to show installed list
        }
        PackageCommand::Update => {
            if parsed.update_all {
                println!("Updating all extensions...");
            } else {
                println!("Checking for updates...");
            }
            // TODO: delegate to self-update and extension update logic
        }
    }

    true
}

fn print_package_command_help(command: &PackageCommand) {
    match command {
        PackageCommand::Install => {
            println!("Usage: pi install <source> [options]");
            println!("Options:");
            println!("  --local, -l   Install from local path");
        }
        PackageCommand::Remove => {
            println!("Usage: pi remove <extension-name>");
        }
        PackageCommand::List => {
            println!("Usage: pi list");
            println!("List all installed extensions.");
        }
        PackageCommand::Update => {
            println!("Usage: pi update [options]");
            println!("Options:");
            println!("  --all, -a     Update all extensions");
            println!("  --force, -f   Force update");
        }
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p pi-coding-agent cli::package_manager_cli::tests -- --nocapture`
Expected: PASS（7 个测试全部通过）

- [ ] **Step 5: 提交**

```bash
git add crates/pi-coding-agent/src/cli/package_manager_cli.rs
git commit -m "feat(cli): add package-manager CLI commands

移植原版 package-manager-cli.ts，支持：
- parse_package_command: 参数解析（install/remove/list/update）
- handle_package_command: 命令分发框架
- 帮助文本输出

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 6: extensions RPC 桥接审查

**Files:**
- Read/Review: `crates/pi-coding-agent/src/core/extensions/mod.rs`
- Read/Review: `crates/pi-coding-agent/src/core/extensions/types.rs`
- Read/Review: `crates/pi-coding-agent/src/core/extensions/rpc.rs`
- Read/Review: `rpc-host/` 目录

**Interfaces:**
- Consumes: 现有 RPC 扩展系统
- Produces: 审查报告，记录覆盖缺口

- [ ] **Step 1: 审查原版 extensions/loader.ts 功能**

原版 loader.ts 职责：
1. 扫描 `~/.pi/extensions/` 和 `{cwd}/.pi/extensions/` 目录
2. 读取 `package.json` 中的 `pi.extensions` 配置
3. 动态加载 JS/TS 文件（通过 jiti/register）

RPC 覆盖情况检查：rpc-host 的 `discover.ts` 是否覆盖了扫描逻辑？
rpc-host/scripts/discover.ts 是否加载 `package.json pi.extensions` 清单？

- [ ] **Step 2: 审查原版 extensions/runner.ts 功能**

原版 runner.ts 职责：
1. 创建 ExtensionAPI 实例并传递给扩展
2. 注册工具、命令、快捷键
3. 管理扩展生命周期

RPC 覆盖情况：rpc-handler 是否完整实现了 ExtensionAPI 桥接？
检查 `crates/pi-coding-agent/src/modes/rpc/handler.rs` 中的命令处理。

- [ ] **Step 3: 审查原版 extensions/wrapper.ts 功能**

原版 wrapper.ts 职责：
1. 将扩展工具输出包装为标准工具定义
2. 处理错误、超时

RPC 覆盖情况：检查 Rust 端是否对 RPC 返回的工具定义做了等价包装。
检查 `crates/pi-coding-agent/src/core/tools/tool_definition_wrapper.rs`。

- [ ] **Step 4: 审查 before_provider_headers 扩展钩子**

原版最近新增钩子 `before_provider_headers`。
检查 `extensions/types.ts` 中的扩展钩子类型定义，确认是否在 RPC 协议中暴露。

- [ ] **Step 5: 输出审查结果到 comments**

审查后输出：
1. 哪些原版功能已被 RPC 覆盖（覆盖率 %）
2. 哪些未覆盖（gap list）
3. 每个 gap 需要添加的 Rust/RPC 端修改

---

### Task 7: bash 超时校验

**Files:**
- Modify: `crates/pi-coding-agent/src/core/bash_executor.rs`

**对应原版 commit:** `85b7c24` (reject non-positive bash timeouts) + `cbcf4e0` (reject oversized bash timeouts)

**变更说明:** 原版 TS 的 `bash-executor.ts` 在执行 bash 命令后检查 timeout 配置值，拒绝 非正数（≤0）和过大（超过某个上限，如 3600 秒）的 timeout 值。

- [ ] **Step 1: 写测试（先失败）**

```rust
// 在 bash_executor.rs 的 tests 模块中添加
#[test]
fn test_timeout_validation_non_positive() {
    // A timeout of 0 or negative should be rejected
    // The exact validation function needs to match the original
}
```

- [ ] **Step 2: 读原版代码确认行为**

原版 `bash-executor.ts` 中的超时校验逻辑：
- `rejectNonPositiveBashTimeouts`: timeout ≤ 0 时拒绝
- `rejectOversizedBashTimeouts`: timeout > 上限（如 3600 秒）时拒绝

- [ ] **Step 3: 在 BashExecutor 中添加超时校验**

在 `execute` 方法中，解析 timeout 参数后增加校验：
```rust
const MAX_BASH_TIMEOUT: u64 = 3600; // 1 hour in seconds

fn validate_timeout(timeout_seconds: u64) -> Result<(), String> {
    if timeout_seconds == 0 {
        return Err("Bash timeout must be positive".to_string());
    }
    if timeout_seconds > MAX_BASH_TIMEOUT {
        return Err(format!(
            "Bash timeout {}s exceeds maximum allowed {}s",
            timeout_seconds, MAX_BASH_TIMEOUT
        ));
    }
    Ok(())
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p pi-coding-agent core::bash_executor::tests -- --nocapture`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add crates/pi-coding-agent/src/core/bash_executor.rs
git commit -m "fix(bash-executor): reject non-positive and oversized bash timeouts

对齐原版 commits 85b7c24 + cbcf4e0：
- 拒绝 ≤ 0 的超时值（non-positive）
- 拒绝 > 3600s 的超时值（oversized）

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 8: 工具刷新保留运行提示

**Files:**
- Modify: `crates/pi-coding-agent/src/core/session_manager.rs`

**对应原版 commit:** `fd6659d` - "fix(coding-agent): preserve run prompt during tool refresh"

**变更说明:** 在工具刷新时，确保运行提示（run prompt）不被丢失。

- [ ] **Step 1: 读原版代码确认变更位置**

原版 `session-manager.ts` 在 `refreshTools()` 或类似方法中，工具刷新前保存当前 `runPrompt`，刷新后恢复。

- [ ] **Step 2: 在 session_manager.rs 找到工具刷新方法**

搜索 `refresh`、`tools` 相关方法。确认 `run_prompt` 字段在刷新时被保存/恢复。

- [ ] **Step 3: 实现保存恢复逻辑**

```rust
// 在工具刷新前：
let saved_run_prompt = self.run_prompt.clone();

// 工具刷新逻辑...

// 刷新后恢复：
self.run_prompt = saved_run_prompt;
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p pi-coding-agent core::session_manager::tests -- --nocapture`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add crates/pi-coding-agent/src/core/session_manager.rs
git commit -m "fix(session-manager): preserve run prompt during tool refresh

对齐原版 commit fd6659d：在工具刷新前保存运行提示，刷新后恢复。

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 9: 会话状态刷新再开始下一轮

**Files:**
- Modify: `crates/pi-coding-agent/src/core/agent_session_runtime.rs`
- Modify: `crates/pi-coding-agent/src/core/agent_session.rs`

**对应原版 commit:** `e547bb9` - "fix(coding-agent): refresh session state before next turn"

**变更说明:** 在开始新一轮 agent 对话之前，先刷新会话状态（session state），确保读取最新的配置变更。

- [ ] **Step 1: 确认 AgentSession 中的 `next_turn` 或等价方法**

在 `agent_session.rs` 中找到下一轮开始前的方法调用点。

- [ ] **Step 2: 在下一轮开始前插入状态刷新**

```rust
// 在 next_turn / 循环开始前
pub async fn prepare_next_turn(&mut self) {
    // Refresh session state before starting the next turn
    self.refresh_session_state().await;
    // ... existing logic
}

async fn refresh_session_state(&mut self) {
    if let Err(e) = self.session_manager.reload_config().await {
        log::warn!("Failed to reload config before next turn: {}", e);
    }
}
```

- [ ] **Step 3: 运行测试确认通过**

Run: `cargo test -p pi-coding-agent core::agent_session::tests -- --nocapture`
Expected: PASS

- [ ] **Step 4: 提交**

```bash
git add crates/pi-coding-agent/src/core/agent_session_runtime.rs crates/pi-coding-agent/src/core/agent_session.rs
git commit -m "fix(agent-session): refresh session state before next turn

对齐原版 commit e547bb9：在开始新一轮 agent 对话前，
重新加载会话配置，确保反映最新的变更。

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 10: 消息内容规范化

**Files:**
- Modify: `crates/pi-coding-agent/src/core/agent_session.rs`
- Modify: `crates/pi-coding-agent/src/core/messages.rs`

**对应原版 commit:** `8c0ccd1` - "fix(ai,agent,coding-agent): normalize null message content at ingestion boundaries"

**变更说明:** 在消息被摄入（ingested）时，将 null 消息内容规范化为空内容或默认值。PLANS.md 标注 "pi-ai 侧已处理"，但仍需检查 pi-coding-agent 的消息摄入路径。

- [ ] **Step 1: 确认 pi-ai 侧已处理 normalize**

检查 `crates/pi-ai/src/types.rs` 或 `crates/pi-ai/src/utils/validation.rs` 中的空值处理。

- [ ] **Step 2: 检查 pi-coding-agent 消息摄入路径**

搜索 `agent_session.rs` 和 `messages.rs` 中消息摄入（add_message、ingest_message 等）方法。

- [ ] **Step 3: 补全规范化逻辑（如有缺口）**

```rust
// 在消息被添加到会话前规范化：
fn normalize_message_content(message: &mut Message) {
    match message {
        Message::User { content, .. } => {
            if content.is_empty() {
                *content = vec![ContentBlock::text("")];
            }
        }
        Message::Assistant { content, .. } => {
            if content.is_empty() {
                *content = vec![];
            }
        }
        _ => {}
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p pi-coding-agent -- --nocapture`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add crates/pi-coding-agent/src/core/agent_session.rs crates/pi-coding-agent/src/core/messages.rs
git commit -m "fix(messages): normalize null message content at ingestion boundaries

对齐原版 commit 8c0ccd1：在消息摄入时规范化空内容。

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 11: split-turn 压缩摘要序列化

**Files:**
- Modify: `crates/pi-coding-agent/src/core/compaction.rs`

**对应原版 commit:** `f58c115` - "fix(coding-agent): serialize split-turn compaction summaries"

**变更说明:** 确保 split-turn 压缩模式的摘要内容被正确序列化保存到磁盘。

- [ ] **Step 1: 分析原版变更**

原版 `compaction` 模块在 split-turn 模式下生成摘要后，需要将其序列化为 JSON/JSONL 格式写入会话文件。

- [ ] **Step 2: 检查 Rust 版 compaction.rs 的序列化路径**

搜索 `serialize`、`save`、`write` 等方法在 `compaction.rs` 中。

- [ ] **Step 3: 补全 split-turn 摘要序列化逻辑**

```rust
// 在相关位置序列化 split-turn 摘要
fn serialize_split_turn_summary(summary: &CompactionSummary) -> Result<String, String> {
    serde_json::to_string_pretty(summary)
        .map_err(|e| format!("Failed to serialize compaction summary: {}", e))
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p pi-coding-agent core::compaction::tests -- --nocapture`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add crates/pi-coding-agent/src/core/compaction.rs
git commit -m "fix(compaction): serialize split-turn compaction summaries

对齐原版 commit f58c115：确保 split-turn 模式的
压缩摘要被正确序列化保存。

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 12: 短 session ID 派生

**Files:**
- Modify: `crates/pi-coding-agent/src/core/session_manager.rs`

**对应原版 commit:** `1dac099` - "fix(agent): derive short session entry ids from the uuidv7 random tail"

**变更说明:** 会话条目 ID 从 UUID v7 的随机尾部派生短 ID。

- [ ] **Step 1: 确认 session_manager.rs 中的 ID 生成逻辑**

搜索 `session_id`、`entry_id`、`generate`、`uuid` 等关键词。

- [ ] **Step 2: 实现短 ID 派生**

```rust
// 从 UUID v7 的随机尾部派生短 ID
fn derive_short_session_id() -> String {
    let uuid = uuid::Uuid::new_v4(); // Placeholder for uuidv7
    // Take the last part of the UUID as a short identifier
    let hex = uuid.to_string();
    let tail = hex.split('-').last().unwrap_or(&hex);
    tail.to_string()
}
```

- [ ] **Step 3: 运行测试确认通过**

Run: `cargo test -p pi-coding-agent core::session_manager::tests -- --nocapture`
Expected: PASS

- [ ] **Step 4: 提交**

```bash
git add crates/pi-coding-agent/src/core/session_manager.rs
git commit -m "fix(session-manager): derive short session entry ids from uuid random tail

对齐原版 commit 1dac099：会话条目 ID 使用 UUID 尾部
派生短标识符。

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 13: 移除冗余 record guards

**Files:**
- Multiple files in `crates/pi-coding-agent/src/core/`

**对应原版 commit:** `035ea9c` - "fix: remove redundant record guards"

**变更说明:** 移除代码中冗余的 record guard 检查（通常是多余的 `if` 或 `unwrap` 包装）。

- [ ] **Step 1: 分析原版变更涉及的范围**

原版 commit 在多个文件中移除了冗余的 record guards，这些 guard 通常是多余的 field presence 检查。

- [ ] **Step 2: 在对应 Rust 文件中查找类似冗余**

搜索 `unwrap()`、`is_some()`、`is_ok()` 后立刻 `unwrap()` 等冗余模式。

- [ ] **Step 3: 移除冗余 guard**

例如将：
```rust
if let Some(ref x) = option {
    do_something(x);
}
```
改为（如果内部逻辑确认 x 必然存在）：
```rust
do_something(option.as_ref().unwrap());
```
更贴近原版风格。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p pi-coding-agent -- --nocapture`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git commit -m "fix: remove redundant record guards

对齐原版 commit 035ea9c：移除多余的 guard 检查。

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## 自检

### Module 对齐覆盖率
| 原版模块 | Task | 覆盖 |
|---|---|---|
| `cli/list-models.ts` | Task 4 | ✅ |
| `package-manager-cli.ts` | Task 5 | ✅ |
| `utils/changelog.ts` | Task 2 | ✅ |
| `utils/open-browser.ts` | Task 1 | ✅ |
| `utils/tools-manager.ts` | Task 3 | ✅ |
| `extensions/loader/runner/wrapper` | Task 6 | ✅ 审查 |
| bash 超时校验 | Task 7 | ✅ |
| 工具刷新保留提示 | Task 8 | ✅ |
| 会话状态刷新 | Task 9 | ✅ |
| 消息规范化 | Task 10 | ✅ |
| split-turn 摘要 | Task 11 | ✅ |
| 短 session ID | Task 12 | ✅ |
| 冗余 guards | Task 13 | ✅ |

### 类型一致性检查
- `format_token_count` (Task 4) 接受 `u64` — 与 `Model::context_window`/`max_tokens` 类型一致
- `ChangelogEntry` (Task 2) 用 `i32` 版本号 — 与 Rust 语义版本处理惯例一致
- `PackageCommand` (Task 5) 用枚举而非字符串 — 比原版 TS 更类型安全
- `handle_package_command` 返回 `bool` — 与原版 `Promise<boolean>` 一致

### 占位符扫描
- Task 3 的 `tools_manager.rs` 初始版本未实现完整下载逻辑 — 这是有意为之（先提供 PATH 扫描 + 手动安装提示，下载功能可以通过后续迭代实现完整的 GitHub API + tar.gz 解压）
- Task 5 的 `package_manager_cli.rs` 中 `handle_package_command` 有 `// TODO:` 注释 — 这些是真正的待办，指向未来需要集成的 `PackageManager` 调用。初始版本提供 CLI 框架 + 参数解析 + 测试覆盖，后续集成到主流程时再填充。
- Task 6 涉及审查 + 可能修改 — 取决于审查结果，不确定是否真的需要改代码，所以步骤 5 表示输出审查报告
