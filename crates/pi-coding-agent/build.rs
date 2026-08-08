//! build.rs — 为 `bun-runtime` feature 生成嵌入的 Bun 二进制。
//!
//! `assets/runtime/` 下的产物按平台命名（xtask `fetch-bun` 下载）：
//! `bun-{os}-{arch}`（darwin-aarch64 / darwin-x64 / linux-aarch64 / linux-x64 /
//! windows-x64 / windows-arm64）。
//!
//! 二进制不进 git（平台相关、61MB，`make fetch-bun` 下载）。存在时生成
//! `include_bytes!` 嵌入；缺失时生成空切片，`bun/mod.rs` 在运行时给出
//! 清晰错误（提示 fetch-bun），而不是编译期硬失败。

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/runtime");

    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let name = match (os.as_str(), arch.as_str()) {
        ("macos", "aarch64") => "bun-darwin-aarch64",
        ("macos", "x86_64") => "bun-darwin-x64",
        ("linux", "aarch64") => "bun-linux-aarch64",
        ("linux", "x86_64") => "bun-linux-x64",
        ("windows", "x86_64") => "bun-windows-x64",
        ("windows", "aarch64") => "bun-windows-arm64",
        _ => {
            println!("cargo:warning=bun-runtime: no Bun binary for {os}/{arch}; run `cargo run -p xtask -- fetch-bun`");
            write_bun_binary(&[]);
            return;
        }
    };

    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/runtime")
        .join(name);
    if path.is_file() {
        // 生成 `pub const BUN_BINARY: &[u8] = include_bytes!("<abs path>");`
        let out = std::env::var("OUT_DIR").expect("OUT_DIR");
        let gen = Path::new(&out).join("bun_binary.rs");
        let abs = path.canonicalize().expect("canonicalize bun binary");
        std::fs::write(
            &gen,
            format!(
                "pub const BUN_BINARY: &[u8] = include_bytes!({:?});\n",
                abs.to_string_lossy()
            ),
        )
        .expect("write bun_binary.rs");
    } else {
        println!(
            "cargo:warning=bun-runtime: {name} missing at {}; run `cargo run -p xtask -- fetch-bun`",
            path.display()
        );
        write_bun_binary(&[]);
    }
}

fn write_bun_binary(bytes: &[u8]) {
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let gen = Path::new(&out).join("bun_binary.rs");
    std::fs::write(&gen, format!("pub const BUN_BINARY: &[u8] = &{bytes:?};\n"))
        .expect("write bun_binary.rs");
}
