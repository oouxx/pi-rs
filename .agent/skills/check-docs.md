---
name: check-docs
description: 检查 pi-rs 仓库文档是否过时、版本口径是否一致、引用是否断裂，并按当前 TS v0.82.1 对齐基准输出维护建议。用于用户要求检查、审查或更新文档时。
---

# 检查 pi-rs 文档

从 pi-rs 仓库根目录执行。当前对齐和验收基准固定为 **TS v0.82.1**；同级 `../pi` 的较新版本只用于识别未来差距，不得把它当作当前对齐目标。

## 检查原则

- 先读取 `MILESTONE.md`，确认当前基准和范围。
- `DEVIATIONS.md` 是有意差异的权威来源；不要把已确认偏差当作 bug。
- `CONTRACT_ALIGNMENT.md` 是公开 API/事件/序列化契约的权威来源。
- `PORTING_MISTAKES.md` 是历史移植错误归档，不因旧版本号自动判定为过时。
- `docs/archive/` 下的文件只作为历史资料，不纳入当前状态判断。
- 只报告与当前代码不一致的内容；不要因为文档日期较早就直接判定过时。
- 未经用户明确要求，不修改文件。

## 执行检查

### 1. 确认版本基准

```bash
grep -nE '对齐/验收基准|当前 TS|Rust workspace|目标版本|v0\.82\.1' \
  MILESTONE.md CONTRACT_ALIGNMENT.md CLAUDE.md

if [ -d ../pi ]; then
  git -C ../pi describe --tags --always
  git -C ../pi show -s --format='%H %s' HEAD
  grep -R '"version"' ../pi/packages/{ai,agent,coding-agent}/package.json | head -20
fi
```

报告必须明确：

```text
当前验收基准：TS v0.82.1
TS 最新参考版本：仅作为未来差距参考
Rust workspace 版本：从 Cargo.toml 读取
```

### 2. 检查已归档文档是否被当作当前文档引用

```bash
grep -RInE \
  'ALIGNMENT_GAPS\.md|ALIGNMENT_V1|GROK_TUI_FEASIBILITY|FEASIBILITY|ACP_ALIGNMENT_AUDIT|GOAL_TS_COMPARISON' \
  --include='*.md' --exclude-dir=.git --exclude-dir=target --exclude-dir=archive . || true
```

发现根目录或 crate 当前文档引用已归档文件时，报告文件、行号和建议的新引用；归档目录内部互相引用不算问题。

### 3. 检查过时的当前状态描述

```bash
grep -RInE \
  '当前范围内不复刻|本轮不复刻|Rust 无 /login|/login.*尚未实现|对齐基准.*v0\.(79|80|81)|当前.*v0\.(79|80|81)|v0\.85\.1' \
  --include='*.md' --exclude-dir=.git --exclude-dir=target --exclude-dir=archive . || true
```

逐项对照源码，不要仅凭 grep 自动修改。特别检查：

- `/login` 是否被描述为完全不存在；当前实际支持 API-key 登录；
- TUI 是否被描述为完全不在范围；当前是“最小可用实现，组件层有意简化”；
- 契约表是否把 v0.80/v0.81/v0.85.1 写成当前基准；
- `已实现`、`未实现`、`范围外` 是否与 `DEVIATIONS.md` 和代码一致。

### 4. 检查未决偏差

```bash
grep -RIn '待确认' --include='DEVIATIONS.md' --exclude-dir=.git --exclude-dir=target . || true
```

将结果分为：

- 仍然真实存在且需要用户决定；
- 已经由代码或用户决定解决，应改为“已确认保留”；
- 仅为历史记录，不应继续放在当前偏差表。

不要自行把“待确认”改成“已确认保留”。

### 5. 检查契约文档覆盖情况

```bash
find crates -name CONTRACT_ALIGNMENT.md -print | sort
find crates -maxdepth 2 -name README.md -print | sort
```

确认每个当前 crate 至少有：

- `CONTRACT_ALIGNMENT.md`（如果有对外 API）；
- `DEVIATIONS.md`（如果存在有意差异）；
- README 只描述用户实际可用入口。

根目录 `CONTRACT_ALIGNMENT.md` 只维护索引和版本口径，不复制各 crate 的完整契约表。

### 6. 检查链接和格式

```bash
git diff --check
find . -name '*.md' -not -path './.git/*' -not -path './target/*' -print0 \
  | xargs -0 grep -nE '\]\([^)]*\.md[^)]*\)' || true
```

人工确认相对链接，尤其是从 `docs/archive/` 和 `crates/*/` 指向根目录的链接。

## 输出格式

按以下结构输出，不要直接大段复制文档：

```text
# 文档检查结果

基准：TS v0.82.1
Rust 版本：...

## 明确过时
- 文件:行号
- 当前文字
- 代码/权威文档实际状态
- 建议动作：更新 / 归档 / 删除

## 仍有待确认
- 文件:行号
- 差异
- 需要用户决定的事项

## 历史内容（无需修改）
- 文件
- 原因

## 链接或结构问题
...

## 结论
- 需要修改的文件：...
- 无需修改的文件：...
```

只有用户随后明确要求“更新文档”时，才执行编辑；编辑前先说明将改哪些文件，编辑后运行 `git diff --check` 和引用检查。
