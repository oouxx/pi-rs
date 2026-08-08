// @earendil-works/pi-tui — 真实 SDK 纯工具 bundle + TUI 组件 stub。
// bundle.js 是 TS 源码转译产物（xtask build-sdk）；TUI 组件是已确认偏差
// （TUI 渲染层未移植），构造时抛错。
export * from "./bundle.js";

function tuiUnavailable(name) {
  throw new Error(
    `TUI component '${name}' is unavailable: pi-tui is not ported in this mode. ` +
    `See DEVIATIONS.md — TUI rendering layer is a confirmed deviation.`
  );
}

export class Box { constructor() { tuiUnavailable("Box"); } }
export class Container { constructor() { tuiUnavailable("Container"); } }
export class Input { constructor() { tuiUnavailable("Input"); } }
export class Text { constructor() { tuiUnavailable("Text"); } }
export class Markdown { constructor() { tuiUnavailable("Markdown"); } }
export class DynamicBorder { constructor() { tuiUnavailable("DynamicBorder"); } }
export class BorderedLoader { constructor() { tuiUnavailable("BorderedLoader"); } }
export class CustomEditor { constructor() { tuiUnavailable("CustomEditor"); } }
export class Editor { constructor() { tuiUnavailable("Editor"); } }
export class SettingsList { constructor() { tuiUnavailable("SettingsList"); } }
export class SelectList { constructor() { tuiUnavailable("SelectList"); } }
export class Spacer { constructor() { tuiUnavailable("Spacer"); } }
export function getMarkdownTheme() { tuiUnavailable("getMarkdownTheme"); }
export function getSettingsListTheme() { tuiUnavailable("getSettingsListTheme"); }
export function fuzzyFilter() { tuiUnavailable("fuzzyFilter"); }
export function wrapTextWithAnsi() { tuiUnavailable("wrapTextWithAnsi"); }
export const Key = new Proxy({}, { get: (t, p) => typeof p === "string" ? () => tuiUnavailable("Key." + p) : undefined });
export const CURSOR_MARKER = "";
