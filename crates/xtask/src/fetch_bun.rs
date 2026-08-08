//! `fetch-bun` — 下载 Bun 运行时二进制到 `assets/runtime/`。
//!
//! 方案 A（子进程 Bun 运行时）的构建期步骤：把 Bun 二进制嵌入发布产物，
//! 运行时解压 spawn，用户环境无需安装 Node/Bun。
//!
//! 用法：
//! ```sh
//! cargo run -p xtask -- fetch-bun            # 下载当前平台 Bun
//! cargo run -p xtask -- fetch-bun --check   # 只校验产物是否存在（CI 用）
//! ```

use std::path::PathBuf;

/// Bun 发布 URL 模板（GitHub releases，zip 包内含单个 `bun` 可执行文件）。
const BUN_VERSION: &str = "1.2.2";
const BUN_URL_TEMPLATE: &str =
    "https://github.com/oven-sh/bun/releases/download/bun-v{VERSION}/bun-{OS}-{ARCH}.zip";

/// 平台 → (OS, ARCH) 映射（Bun 的命名）。
fn bun_platform() -> anyhow::Result<(&'static str, &'static str)> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match (os, arch) {
        ("macos", "aarch64") => Ok(("darwin", "aarch64")),
        ("macos", "x86_64") => Ok(("darwin", "x64")),
        ("linux", "aarch64") => Ok(("linux", "aarch64")),
        ("linux", "x86_64") => Ok(("linux", "x64")),
        ("windows", "x86_64") => Ok(("windows", "x64")),
        ("windows", "aarch64") => Ok(("windows", "arm64")),
        _ => anyhow::bail!("unsupported platform for Bun: {os}/{arch}"),
    }
}

/// 仓库内 `assets/runtime` 目录（产物落盘处，提交进 git 或 CI 产物）。
fn runtime_dir() -> anyhow::Result<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest
        .parent() // crates/
        .ok_or_else(|| anyhow::anyhow!("cannot locate crates/ parent of xtask"))?
        .parent() // 仓库根
        .ok_or_else(|| anyhow::anyhow!("cannot locate repo root"))?
        .join("assets")
        .join("runtime");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// 目标文件名：`bun-{os}-{arch}`（与 `bun_platform` 对应）。
fn target_name() -> anyhow::Result<String> {
    let (os, arch) = bun_platform()?;
    Ok(format!("bun-{os}-{arch}"))
}

pub fn run(check_only: bool) -> anyhow::Result<()> {
    let dir = runtime_dir()?;
    let name = target_name()?;
    let out = dir.join(&name);

    if out.exists() {
        println!("✓ {name} already present at {}", out.display());
        return Ok(());
    }
    if check_only {
        anyhow::bail!("{name} missing at {} (run `cargo run -p xtask -- fetch-bun`)", out.display());
    }

    let (os, arch) = bun_platform()?;
    let url = BUN_URL_TEMPLATE
        .replace("{VERSION}", BUN_VERSION)
        .replace("{OS}", os)
        .replace("{ARCH}", arch);
    println!("↓ downloading {url}");

    let resp = reqwest::blocking::get(&url)?.error_for_status()?;
    let bytes = resp.bytes()?;
    println!("  got {} bytes", bytes.len());

    // zip 包内含单个 `bun`（或 `bun.exe`）可执行文件，解压取出。
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let mut found = None;
    for i in 0..archive.len() {
        let mut f = archive.by_index(i)?;
        let fname = f.name().to_string();
        if fname.ends_with("bun") || fname.ends_with("bun.exe") {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut f, &mut buf)?;
            found = Some(buf);
            break;
        }
    }
    let Some(bin) = found else {
        anyhow::bail!("bun executable not found in zip archive");
    };

    std::fs::write(&out, &bin)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755))?;
    }
    println!("✓ wrote {} ({} bytes)", out.display(), bin.len());
    Ok(())
}
