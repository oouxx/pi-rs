// Minimal bootstrap: load extension, provide pi API backed by stdio JSON-RPC.
import { createInterface } from "node:readline";

const extensionPath = process.argv[2];
if (!extensionPath) { console.error("usage: bun bootstrap.ts <extension-path>"); process.exit(1); }

// ── stdio JSON-RPC ────────────────────────────────────────────────
let nextId = 1;
const pending = new Map();
const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });

function send(obj) { process.stdout.write(JSON.stringify(obj) + "\n"); }

function request(method, params) {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    send({ id, method, params });
  });
}

rl.on("line", async (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  if (msg.id !== undefined && pending.has(msg.id)) {
    const p = pending.get(msg.id);
    pending.delete(msg.id);
    if (msg.error) p.reject(new Error(msg.error.message || String(msg.error)));
    else p.resolve(msg.result);
    return;
  }
  // Host → Bun request (execute_tool / fire_event / execute_command)
  if (msg.method === "execute_tool") {
    const { name, callId, params } = msg.params;
    try {
      const fn = toolExecutors.get(name);
      if (!fn) throw new Error("Tool not found: " + name);
      const result = await fn(callId, params, undefined, undefined, createCtx());
      send({ id: msg.id, result });
    } catch (e) {
      send({ id: msg.id, error: { message: String(e && e.message || e) } });
    }
  } else if (msg.method === "fire_event") {
    const { event, data } = msg.params;
    const results = [];
    for (const h of (handlers.get(event) || [])) {
      const r = await h(data, createCtx());
      if (r !== undefined) results.push(r);
    }
    send({ id: msg.id, result: results });
  } else if (msg.method === "execute_command") {
    const { name } = msg.params;
    try {
      const fn = commandHandlers.get(name);
      if (fn) await fn();
      send({ id: msg.id, result: null });
    } catch (e) {
      send({ id: msg.id, error: { message: String(e && e.message || e) } });
    }
  } else if (msg.method === "shutdown") {
    process.exit(0);
  }
});

// ── pi API ─────────────────────────────────────────────────────────
const handlers = new Map();
const toolExecutors = new Map();
const commandHandlers = new Map();
const shortcutHandlers = new Map();
const flagValues = new Map();

// CLI 传入的 flag（宿主经 PI_EXTENSION_FLAGS 环境变量下发），覆盖
// registerFlag 默认值。
try {
  const cliFlags = JSON.parse(process.env.PI_EXTENSION_FLAGS || "{}");
  for (const [k, v] of Object.entries(cliFlags)) flagValues.set(k, v);
} catch {}

function createCtx() {
  return {
    ui: { notify: (m) => send({ method: "log", params: { message: String(m) } }), setStatus: () => {}, confirm: () => Promise.resolve(false), select: () => Promise.resolve(undefined), input: () => Promise.resolve(undefined) },
    mode: "fast", hasUI: false, cwd: process.env.PI_SESSION_CWD || process.cwd(),
    sessionManager: new Proxy({}, { get: () => () => {} }),
    modelRegistry: new Proxy({}, { get: () => () => {} }),
    model: undefined, scopedModels: [], thinkingLevel: undefined,
    isIdle: () => true, isProjectTrusted: () => false,
    signal: undefined, abort: () => {}, hasPendingMessages: () => false,
    shutdown: () => {}, getContextUsage: () => undefined, compact: () => {}, getSystemPrompt: () => "",
  };
}

const pi = {
  log(msg) { send({ method: "log", params: { message: String(msg) } }); },
  on(event, handler) {
    const list = handlers.get(event) || [];
    list.push(handler);
    handlers.set(event, list);
    send({ method: "register_handler", params: { event } });
  },
  registerTool(tool) {
    toolExecutors.set(tool.name, tool.execute);
    send({ method: "register_tool", params: { name: tool.name, description: tool.description || "", parameters: tool.parameters ? JSON.stringify(tool.parameters) : null } });
  },
  registerCommand(name, options = {}) {
    commandHandlers.set(name, options.handler);
    send({ method: "register_command", params: { name, description: options.description || null, subcommands: JSON.stringify(options.subcommands || []) } });
  },
  registerShortcut(shortcut, options = {}) {
    shortcutHandlers.set(String(shortcut), options.handler);
    send({ method: "register_shortcut", params: { shortcut: String(shortcut), description: options.description || null } });
  },
  registerFlag(name, options = {}) {
    const def = options.default !== undefined ? String(options.default) : null;
    if (def !== null && !flagValues.has(name)) flagValues.set(name, options.default);
    send({ method: "register_flag", params: { name: String(name), flag_type: String(options.type || "boolean"), description: options.description || null, default_value: def } });
  },
  getFlag(name) {
    const v = flagValues.get(String(name));
    if (v === "true") return true;
    if (v === "false") return false;
    return v;
  },
  registerMessageRenderer(customType) { send({ method: "register_message_renderer", params: { custom_type: String(customType) } }); },
  registerMarkdownTransformer() {},
  registerEntryRenderer(customType) { send({ method: "register_entry_renderer", params: { custom_type: String(customType) } }); },
  sendMessage(message, options) { return request("send_message", { message_json: JSON.stringify(message), options_json: options ? JSON.stringify(options) : null }); },
  sendUserMessage(content, options) { return request("send_user_message", { content: typeof content === "string" ? content : JSON.stringify(content), options_json: options ? JSON.stringify(options) : null }); },
  appendEntry(customType, data) { return request("append_entry", { custom_type: String(customType), data_json: data !== undefined ? JSON.stringify(data) : null }); },
  setSessionName(name) { return request("set_session_name", { name: String(name) }); },
  getSessionName() { return request("get_session_name", {}); },
  setLabel(entryId, label) { return request("set_label", { entry_id: String(entryId), label: label != null ? String(label) : null }); },
  getActiveTools() { return request("get_active_tools", {}); },
  getAllTools() { return request("get_all_tools", {}); },
  setActiveTools(toolNames) { return request("set_active_tools", { tools_json: JSON.stringify(toolNames) }); },
  getCommands() { return request("get_commands", {}); },
  setModel(model) { const id = (model && (model.provider + "/" + model.id)) || String(model); return request("set_model", { model_id: String(id) }); },
  exec(command, args, options) { return request("exec", { command: String(command), args_json: JSON.stringify(args || []), options_json: options ? JSON.stringify(options) : null }); },
  getThinkingLevel() { return request("get_thinking_level", {}); },
  setThinkingLevel(level) { return request("set_thinking_level", { level: String(level) }); },
  registerProvider(name, config) { return request("register_provider", { name: String(name), config_json: JSON.stringify(config), extension_path: String(process.env.PI_EXTENSION_PATH || "<unknown>") }); },
  unregisterProvider(name) { return request("unregister_provider", { name: String(name) }); },
  events: {
    on(event, handler) { const list = handlers.get(event) || []; list.push(handler); handlers.set(event, list); },
    emit(event, ...args) { for (const h of (handlers.get(event) || [])) h(...args); },
    off(event, handler) { const list = handlers.get(event); if (!list) return; if (handler) handlers.set(event, list.filter(h => h !== handler)); else handlers.delete(event); },
  },

  // 内部：工具工厂（createBashTool 等）的 execute 经此 RPC 到宿主跑真实内置工具。
  __runBuiltinTool(name, params) {
    return request("run_builtin_tool", { name: String(name), params_json: JSON.stringify(params || {}) });
  },

  // 内部：pi-ai shim 的 complete/streamSimple 经此 RPC 到宿主跑补全。
  __piAiComplete(requestObj) {
    return request("pi_ai_complete", { request_json: JSON.stringify(requestObj || {}) });
  },

  // 内部：pi-ai shim 的 getModel 经此 RPC 到宿主取当前模型。
  __piGetModel() {
    return request("get_model", {});
  },

  // 内部：pi-ai shim 的 registerApiProvider 经此 RPC 到宿主注册 provider。
  __piRegisterProvider(name, config) {
    return request("register_provider", {
      name: String(name),
      config_json: JSON.stringify(config || {}),
      extension_path: String(process.env.PI_EXTENSION_PATH || "<unknown>"),
    });
  },
};

// 挂到 globalThis，供 SDK shim 的工具工厂（createBashTool 等）在 execute
// 时经 `globalThis.__pi.__runBuiltinTool` 回调宿主。
globalThis.__pi = pi;

// ── Load extension ──────────────────────────────────────────────────
const mod = await import(extensionPath);
const factory = mod.default;
await factory(pi);
send({ method: "loaded", params: {} });
