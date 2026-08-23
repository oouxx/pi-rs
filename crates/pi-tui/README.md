# pi-tui

Rust 移植版终端 UI 框架，对应 TypeScript 原版 `@earendil-works/pi-tui`。

基于 `ratatui` 渲染，提供 `Component` trait 驱动组件化 UI。

## Features

- **Component-based**: 统一的 `Component` trait（`render()` / `handle_input()` / `invalidate()`）
- **Overlay 系统**: 叠加层支持锚点定位（9 个方向）、偏移、边距、隐藏/显示
- **硬件光标定位**: 通过 `CURSOR_MARKER` 零宽标记在输出中标记光标位置，支持 IME
- **内置组件**: Text, TruncatedText, Input, Editor, Markdown, Loader, CancellableLoader, SelectList, SettingsList, Spacer, Image, Box, Container
- **终端内联图片**: 基于 `ratatui-image`，支持 Kitty / iTerm2 / Sixel / Halfblocks 协议
- **Autocomplete**: 文件路径补全 + 斜杠命令 + @前缀
- **模糊搜索**: 子序列匹配，多 token，排序
- **238 个单元测试**，全量通过

## Quick Start

```rust
use pi_tui::{
    Tui, Terminal, Component, TextComponent, Editor,
    keybindings::Key,
};

// Terminal 初始化
let mut terminal = Terminal::new()?;
let (input_rx, _guard) = terminal.start()?;

// 创建 TUI
let mut tui = Tui::new(terminal);

// 添加组件
tui.add_child(Box::new(TextComponent::new("Welcome!")));

// 启动事件循环（需自行实现，见 examples/）
```

## Core API

### Component Trait

所有 UI 组件实现：

```rust
pub trait Component: Send + Sync {
    fn render(&self, width: u16) -> Vec<String>;
    fn handle_input(&mut self, _data: &str) {}
    fn wants_key_release(&self) -> bool { false }
    fn invalidate(&mut self) {}
}
```

### TUI

```rust
let mut tui = Tui::new(terminal);

tui.add_child(component);
tui.remove_child(&component);
tui.set_focus_index(index);

// 渲染管线
let (lines, cursor) = tui.render_to_lines(width, height);

// 请求重绘
tui.request_render(force)?;
```

### Overlays

```rust
let mut handle = tui.show_overlay(
    Box::new(my_component),
    OverlayOptions {
        width: Some(60),
        min_width: Some(40),
        max_height: Some(20),
        anchor: OverlayAnchor::Center,
        offset_x: 2,
        offset_y: -1,
        margin: OverlayMargin { top: 1, right: 2, bottom: 1, left: 2 },
    },
);

handle.hide();              // 永久移除
handle.set_hidden(true);    // 临时隐藏
handle.set_hidden(false);   // 重新显示
handle.is_hidden();         // 查询状态
tui.hide_overlays();        // 隐藏所有覆盖层
```

锚点值：`Center`, `TopLeft`, `TopCenter`, `TopRight`, `CenterLeft`, `CenterRight`, `BottomLeft`, `BottomCenter`, `BottomRight`

### Focusable Trait（IME 支持）

```rust
pub trait Focusable {
    fn focused(&self) -> bool;
    fn set_focused(&mut self, focused: bool);
}
```

组件渲染输出中嵌入 `CURSOR_MARKER`（零宽 APC 序列 `\x1b_pi:c\x07`），TUI 渲染管线会提取并设置硬件光标位置。

## Built-in Components

### Container

```rust
let mut container = Container::new();
container.add_child(Box::new(my_component));
container.remove_child(&component);
container.clear();
container.render(80); // Vec<String>
```

### Box (BoxComponent)

```rust
let mut bx = BoxComponent::new(1, 1, None);
bx.set_bg_fn(|text| format!("\x1b[48;5;240m{}\x1b[0m", text));
bx.add_child(Box::new(TextComponent::new("Content")));
```

### Text (TextComponent)

```rust
let mut text = TextComponent::new("Hello World");
text.set_text("Updated");
text.set_padding(1, 1);
text.set_custom_bg_fn(|s| format!("\x1b[44m{}\x1b[0m", s));
```

### TruncatedText

```rust
let truncated = TruncatedText::new("Very long text...", 0, 0);
```

### Input

```rust
let mut input = Input::new();
input.set_value("initial");
input.get_value();       // 当前文本
input.handle_input("\r"); // 提交
```

快捷键：`Enter`(提交), `Ctrl+A/E`(行首/尾), `Ctrl+W`(删词后退), `Ctrl+U/K`(删至行首/尾), `Ctrl/Alt+Left/Right`(词导航)

### Editor

```rust
let mut editor = Editor::new(tui, theme, EditorOptions::default());
editor.set_autocomplete_provider(provider);
editor.set_disable_submit(true);
editor.set_border_color(|s| format!("\x1b[34m{}\x1b[0m", s));
```

快捷键：`Enter`(提交), `Shift/Ctrl/Alt+Enter`(换行), `Tab`(自动补全), `Ctrl+K/U/W`(删除), `Ctrl+A/E`(行首/尾)

### Markdown

```rust
let md = Markdown::new(
    "# Hello\n\n**bold** text",
    1, 1,
    markdown_theme,
    Some(default_style),
);
```

### Loader

```rust
let mut loader = Loader::new(
    None,
    Box::new(|s| format!("\x1b[36m{}\x1b[0m", s)),  // spinner color
    Box::new(|s| format!("\x1b[2m{}\x1b[0m", s)),   // message color
    "Loading...",
    None,
);
loader.set_message("Processing...");
loader.advance_frame();
```

### CancellableLoader

```rust
let mut loader = CancellableLoader::new(
    spinner_color_fn,
    message_color_fn,
    "Working...",
    None,
);
loader.set_on_abort(Box::new(|| {
    // 用户按 Escape 时调用
}));
loader.signal();  // 触发取消
loader.aborted()  // 是否已取消
```

### SelectList

```rust
let mut list = SelectList::new(
    items,
    5,
    select_list_theme,
);
list.set_filter("opt");
```

### SettingsList

```rust
let mut settings = SettingsList::new(
    setting_items,
    10,
    settings_theme,
    Box::new(|id, val| println!("{} changed to {}", id, val)),
    Box::new(|| println!("cancelled")),
    SettingsListOptions { enable_search: false },
);
settings.update_value("theme", "light");
```

### Spacer

```rust
let spacer = Spacer::new(2);
```

### ImageComponent

```rust
let img = ImageComponent::from_path(
    "path/to/image.png",
    &mut picker,
    image_theme,
    ImageOptions { max_width_cells: Some(40), max_height_cells: None, filename: None },
);
```

支持 PNG/JPEG/GIF/WebP，自动检测 Kitty/iTerm2/Sixel/Halfblocks 协议。

### CURSOR_MARKER

常量 `"\x1b_pi:c\x07"`，组件可在 `render()` 输出中嵌入此标记，
渲染管线会自动提取并设置硬件光标位置。

## Utilities

```rust
use pi_tui::utils::{visible_width, truncate_to_width, wrap_text_with_ansi};

let width = visible_width("\x1b[31mHello\x1b[0m"); // 5
let truncated = truncate_to_width("Hello World", 8); // "Hello..."
let lines = wrap_text_with_ansi("long text", 20);
```

## Keys

```rust
use pi_tui::keys::{matches_key, parse_key, Key};

if matches_key(data, Key::ctrl("c")) { /* exit */ }
if matches_key(data, "\r") { /* submit */ }
if matches_key(data, "\x1b[A") { /* up */ }
```

## Autocomplete

```rust
use pi_tui::CombinedAutocompleteProvider;

let provider = CombinedAutocompleteProvider::new(
    vec![
        AutocompleteItem { name: "help".into(), description: Some("Show help".into()) },
        AutocompleteItem { name: "clear".into(), description: Some("Clear".into()) },
    ],
    "/path/to/base",
);
```

## Number of Tests

238 个单元测试覆盖所有模块。运行：

```bash
cargo test -p pi-tui
```

## Crate 架构

```
src/
├── lib.rs                 # barrel 导出
├── tui.rs                 # TUI 主类、Container、Overlay、Component trait（671 行, 39 tests）
├── terminal.rs            # Terminal 封装（crossterm + ratatui）
├── components/
│   ├── mod.rs
│   ├── spacer.rs
│   ├── text.rs
│   ├── truncated_text.rs  # ✅ 已完成
│   ├── input.rs
│   ├── editor.rs          # ✅ 多行编辑器，1,838 行
│   ├── markdown.rs        # ✅ pulldown-cmark 渲染
│   ├── select_list.rs
│   ├── settings_list.rs   # ✅ 已完成
│   ├── loader.rs          # ✅ 已完成
│   ├── cancellable_loader.rs # ✅ 已完成
│   ├── image.rs           # ✅ ratatui-image 集成
│   └── box_component.rs
├── autocomplete.rs        # 路径补全 + 斜杠命令 + @前缀
├── editor_component.rs    # Editor 插件接口
├── native_modifiers.rs    # macOS 修饰键检测（CoreGraphics FFI）
├── fuzzy.rs               # 模糊搜索
├── keybindings.rs         # 快捷键系统
├── keys.rs                # 按键解析
├── kill_ring.rs           # Emacs kill-ring
├── stdin_buffer.rs        # 终端输入缓冲
├── undo_stack.rs          # 泛型撤销栈
├── utils.rs               # strip_ansi / visible_width / truncate 等
└── word_navigation.rs     # 单词级导航
```
