//! build.rs 退化为不联网。
//!
//! 内置模型列表由维护者跑 `cargo run -p xtask -- generate-models` 预先生成，
//! 落盘到 `data/models_generated.json` 并提交进仓，运行时由
//! `register_builtins` 通过 `include_str!` 嵌入。此处只声明文件依赖，保证
//! 产物或本脚本变更时 cargo 重新触发 `include_str!` 的重新编译。
//!
//! 对应原版 pi 的设计：生成是维护者动作（`scripts/generate-models.ts`），
//! 编译只读已生成的产物，下游零网络、可复现。

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=data/models_generated.json");
}
