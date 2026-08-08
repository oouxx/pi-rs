//! `build-sdk` — 用 Bun 打包 TS 仓库的真实 SDK 纯函数到 `assets/sdk/`。
//!
//! 替换手写 shim：扩展 import 的 `@earendil-works/pi-*` 纯函数（StringEnum/
//! isContextOverflow/uuidv7/parseFrontmatter/convertToLlm/serializeConversation/
//! withFileMutationQueue/stripTerminalSequences/visibleWidth/...）直接来自
//! TS 源码转译产物，不再手写。RPC 桥接函数（complete/getModel/工具工厂）
//! 由运行时 wrapper 叠加。
//!
//! 依赖：TS 仓库在 `../pi`（与 pi-rs 同级），本机有 `bun`。
//!
//! 用法：
//! ```sh
//! cargo run -p xtask -- build-sdk
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

/// 输出目录：`assets/sdk/`。
fn sdk_dir() -> anyhow::Result<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest
        .parent() // crates/
        .ok_or_else(|| anyhow::anyhow!("cannot locate crates/ parent of xtask"))?
        .parent() // 仓库根
        .ok_or_else(|| anyhow::anyhow!("cannot locate repo root"))?
        .join("assets")
        .join("sdk");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// TS 仓库路径（与 pi-rs 同级）。
fn ts_repo() -> anyhow::Result<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest
        .parent() // crates/
        .ok_or_else(|| anyhow::anyhow!("cannot locate crates/ parent of xtask"))?
        .parent() // pi-rs
        .ok_or_else(|| anyhow::anyhow!("cannot locate pi-rs root"))?
        .parent() // github/
        .ok_or_else(|| anyhow::anyhow!("cannot locate github/"))?
        .join("pi");
    if !repo.join("packages").is_dir() {
        anyhow::bail!("TS repo not found at {} (expected sibling `pi` of pi-rs)", repo.display());
    }
    Ok(repo)
}

/// 在 TS 仓库里写一个临时入口文件，跑 bun build，输出到 assets/sdk/。
fn build_entry(
    ts_repo: &Path,
    entry_rel: &str,
    entry_content: &str,
    out_name: &str,
    external: &[&str],
) -> anyhow::Result<()> {
    let entry = ts_repo.join(entry_rel);
    std::fs::write(&entry, entry_content)?;

    let out = sdk_dir()?.join(out_name);
    let mut cmd = Command::new("bun");
    cmd.args(["build", entry_rel, "--target=bun", "--format=esm", "--outfile"]);
    cmd.arg(&out);
    for ext in external {
        cmd.arg("--external").arg(ext);
    }
    cmd.current_dir(ts_repo);
    let status = cmd.status().map_err(|e| anyhow::anyhow!("bun build failed: {e}"))?;
    if !status.success() {
        anyhow::bail!("bun build {entry_rel} failed");
    }
    let _ = std::fs::remove_file(&entry);
    println!("✓ {out_name} ({} bytes)", std::fs::metadata(&out)?.len());
    Ok(())
}

pub fn run() -> anyhow::Result<()> {
    let ts = ts_repo()?;
    println!("TS repo: {}", ts.display());

    // pi-ai 纯函数（typebox 外部，运行时由 workspace node_modules 提供真实包）
    build_entry(
        &ts,
        "sdk-entry-pi-ai.ts",
        r#"
import { StringEnum } from "./packages/ai/src/utils/typebox-helpers.ts";
import { isContextOverflow } from "./packages/ai/src/utils/overflow.ts";
import { isRetryableAssistantError } from "./packages/ai/src/utils/retry.ts";
import { uuidv7 } from "./packages/ai/src/utils/uuid.ts";
import { Type } from "typebox";
export { StringEnum, isContextOverflow, isRetryableAssistantError, uuidv7, Type };
"#,
        "pi-ai-bundle.js",
        &["typebox"],
    )?;

    // pi-coding-agent 纯函数
    build_entry(
        &ts,
        "sdk-entry-pi-ca.ts",
        r#"
import { parseFrontmatter, stripFrontmatter } from "./packages/coding-agent/src/utils/frontmatter.ts";
import { convertToLlm } from "./packages/coding-agent/src/core/messages.ts";
import { serializeConversation } from "./packages/coding-agent/src/core/compaction/utils.ts";
import { withFileMutationQueue } from "./packages/coding-agent/src/core/tools/file-mutation-queue.ts";
export { parseFrontmatter, stripFrontmatter, convertToLlm, serializeConversation, withFileMutationQueue };
"#,
        "pi-coding-agent-bundle.js",
        &[],
    )?;

    // pi-tui 纯工具
    build_entry(
        &ts,
        "sdk-entry-pi-tui.ts",
        r#"
import { stripTerminalSequences, extractAnsiCode, visibleWidth, truncateToWidth } from "./packages/tui/src/utils.ts";
import { matchesKey, isKeyRelease, parseKey } from "./packages/tui/src/keys.ts";
export { stripTerminalSequences, extractAnsiCode, visibleWidth, truncateToWidth, matchesKey, isKeyRelease, parseKey };
"#,
        "pi-tui-bundle.js",
        &[],
    )?;

    // 真实 typebox 包（从 packages/ai/node_modules 复制，只留运行时文件）
    let typebox_src = ts.join("packages/ai/node_modules/typebox");
    if !typebox_src.is_dir() {
        anyhow::bail!("typebox not found at {}", typebox_src.display());
    }
    let typebox_dst = sdk_dir()?.join("typebox");
    let _ = std::fs::remove_dir_all(&typebox_dst);
    copy_dir_runtime(&typebox_src, &typebox_dst)?;
    println!("✓ typebox/ (copied from TS repo, runtime files only)");

    println!("SDK bundles written to {}", sdk_dir()?.display());
    Ok(())
}

fn copy_dir(src: &Path, dst: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// 复制目录，只保留运行时文件（.mjs/.js/.json/license/readme），跳过
/// .d.ts/.d.mts 类型声明（Bun 运行时不需要）。
fn copy_dir_runtime(src: &Path, dst: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy().to_string();
        if name_str.ends_with(".d.ts") || name_str.ends_with(".d.mts") {
            continue;
        }
        let target = dst.join(&name);
        if entry.file_type()?.is_dir() {
            copy_dir_runtime(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
