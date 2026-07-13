// pi extension runtime shim -- loaded as the entry ESM module of the embedded
// deno_core runtime. Defines globalThis.pi (the ExtensionAPI surface), the
// registry Maps that hold JS-side state (tool handlers, event handlers, etc.),
// and the dispatch functions Rust calls back into (__piDispatch,
// __piDispatchResult, __piCallTool).
//
// Extension files are dynamically imported from here; each must default-export
// a factory `function(pi) { ... }` that registers tools / event hooks.

const tools = new Map();        // name -> { definition, handler }
const commands = new Map();     // name -> { description? }
const flags = new Map();         // name -> { description?, type, default? }
const flagValues = new Map();    // name -> boolean|string
const shortcuts = new Map();     // key -> { description? }
const handlers = new Map();      // eventType -> Array<(event, ctx) => any>
const pendingNotifications = []; // populated by ctx.ui.notify; returned per tool call
let sessionCwd = "/";            // set by Rust at load time; default cwd for pi.exec

// Merge a caller's exec options with the default cwd, letting an explicit
// options.cwd override the default (mirrors original pi: options?.cwd ?? runner.cwd).
function execWithDefaultCwd(command, args, options, defaultCwd) {
  const opts = options ?? {};
  if (opts.cwd === undefined || opts.cwd === null) opts.cwd = defaultCwd;
  return Deno.core.ops.op_pi_exec(command, args ?? [], opts);
}

// Context mode state - set by Rust via __piSetContextMode.
let contextMode = "rpc";
let contextHasUI = false;

globalThis.__piSetContextMode = (mode, hasUI) => {
  contextMode = mode ?? "rpc";
  contextHasUI = hasUI === true;
};

function makeContext(cwd) {
  const ctxCwd = cwd ?? sessionCwd ?? "/";
  const notSupported = (name) => () => {
    throw new Error(`ctx.${name} is not yet supported by the embedded runtime`);
  };
  return {
    cwd: ctxCwd,
    mode: contextMode,
    hasUI: contextHasUI,
    ui: {
      // Notifications are buffered JS-side so __piCallTool can return them; the
      // Rust op_pi_notify mirrors into OpState for any Rust-side consumer too.
      notify: (msg, type) => {
        pendingNotifications.push(msg);
        try { Deno.core.ops.op_pi_notify(msg, type); } catch {}
      },
      setStatus: (key, text) => {
        try { Deno.core.ops.op_pi_ui_set_status(key, text); } catch {}
      },
    },
    exec: (command, args, options) =>
      execWithDefaultCwd(command, args, options, ctxCwd),
    // Context action methods - call through to Rust ops (stubs return defaults
    // until RuntimeCommand variants are added for real host state).
    isIdle: () => { try { return Deno.core.ops.op_pi_ctx_is_idle(); } catch { return true; } },
    isProjectTrusted: () => { try { return Deno.core.ops.op_pi_ctx_is_project_trusted(); } catch { return true; } },
    abort: () => { try { Deno.core.ops.op_pi_ctx_abort(); } catch {} },
    hasPendingMessages: () => { try { return Deno.core.ops.op_pi_ctx_has_pending_messages(); } catch { return false; } },
    shutdown: () => { try { Deno.core.ops.op_pi_ctx_shutdown(); } catch {} },
    getSystemPrompt: () => { try { return Deno.core.ops.op_pi_ctx_get_system_prompt(); } catch { return ""; } },
    // Missing ExtensionContext methods - push HostCommand for main-thread processing.
    sessionManager: {
      newSession: (options) => { try { Deno.core.ops.op_pi_new_session(options ?? {}); } catch {} },
      fork: (entryId, options) => { try { Deno.core.ops.op_pi_fork(entryId, options ?? {}); } catch {} },
      switchSession: (sessionPath, options) => { try { Deno.core.ops.op_pi_switch_session(sessionPath, options ?? {}); } catch {} },
      reload: () => { try { Deno.core.ops.op_pi_reload(); } catch {} },
    },
    modelRegistry: {
      getModel: () => { try { return Deno.core.ops.op_pi_ctx_get_model(); } catch { return null; } },
      setModel: (model) => { try { Deno.core.ops.op_pi_set_model(model.id ?? model); } catch {} },
    },
    model: null,
    signal: null,
    getContextUsage: () => { try { return Deno.core.ops.op_pi_ctx_get_context_usage(); } catch { return { tokensUsed: 0, tokensTotal: 0, percentUsed: 0 }; } },
    compact: () => { try { Deno.core.ops.op_pi_ctx_compact(); } catch {} },
    // ExtensionCommandContext methods -- call through to Rust ops.
    waitForIdle: () => { try { Deno.core.ops.op_pi_wait_for_idle(); } catch {} },
    newSession: (options) => { try { Deno.core.ops.op_pi_new_session(options ?? {}); } catch {} },
    fork: (entryId, options) => { try { Deno.core.ops.op_pi_fork(entryId, options ?? {}); } catch {} },
    navigateTree: (direction) => { try { Deno.core.ops.op_pi_navigate_tree(direction); } catch {} },
    switchSession: (sessionPath, options) => { try { Deno.core.ops.op_pi_switch_session(sessionPath, options ?? {}); } catch {} },
    reload: () => { try { Deno.core.ops.op_pi_reload(); } catch {} },
    getSystemPromptOptions: notSupported("getSystemPromptOptions"),
  };
}

globalThis.__piMakeContext = makeContext;

// Rust sets the session cwd before loading extensions so pi.exec defaults to it.
globalThis.__piSetCwd = (cwd) => { sessionCwd = cwd ?? "/"; };

// Build the per-extension pi API. Each extension's factory receives one.
function makePi() {
  const notSupported = (name) => () => {
    throw new Error(`pi.${name} is not yet supported by the embedded runtime`);
  };
  return {
    // Mirrors ctx.ui so extensions can call pi.ui.notify (the documented API)
    // as well as ctx.ui.notify from inside a tool execute handler.
    ui: {
      notify: (msg, type) => {
        pendingNotifications.push(msg);
        try { Deno.core.ops.op_pi_notify(msg, type); } catch {}
      },
      setStatus: () => {},
    },
    registerTool: (tool) => {
      // Full tool (with JS execute handler) stays in JS; metadata goes to Rust.
      tools.set(tool.name, tool);
      Deno.core.ops.op_pi_register_tool({
        name: tool.name,
        description: tool.description ?? "",
        parameters: tool.parameters ?? null,
        prompt_guidelines: tool.promptGuidelines ?? null,
        execution_mode: tool.executionMode ?? null,
      });
    },
    registerCommand: (name, options) => {
      commands.set(name, options);
      Deno.core.ops.op_pi_register_command(name, options);
    },
    registerShortcut: (key, options) => {
      shortcuts.set(key, options);
      Deno.core.ops.op_pi_register_shortcut(key, options);
    },
    registerFlag: (name, options) => {
      flags.set(name, options);
      if (options && options.default !== undefined && !flagValues.has(name)) {
        flagValues.set(name, options.default);
      }
      Deno.core.ops.op_pi_register_flag(name, options);
    },
    getFlag: (name) => flagValues.has(name) ? flagValues.get(name) : undefined,
    getCommands: () => Deno.core.ops.op_pi_get_commands(),
    on: (eventType, handler) => {
      let list = handlers.get(eventType);
      if (!list) { list = []; handlers.set(eventType, list); }
      list.push(handler);
    },
    exec: (command, args, options) =>
      execWithDefaultCwd(command, args, options, sessionCwd),
    // In-process EventEmitter for cross-extension communication (Task 5.9)
    events: (() => {
      const eventBusHandlers = new Map();
      return {
        emit: (event, data) => {
          const list = eventBusHandlers.get(event);
          if (!list) return;
          for (const h of list) {
            try { h(data); } catch (e) { try { Deno.core.ops.op_pi_log(String(e && e.stack || e)); } catch {} }
          }
        },
        on: (event, handler) => {
          let list = eventBusHandlers.get(event);
          if (!list) { list = []; eventBusHandlers.set(event, list); }
          list.push(handler);
          return () => { const idx = list.indexOf(handler); if (idx >= 0) list.splice(idx, 1); };
        },
      };
    })(),
    // Phase 5.4-5.6: ops that throw "not supported" until RuntimeCommand variants are added
    registerProvider: (name, config) => Deno.core.ops.op_pi_register_provider(name, config),
    unregisterProvider: (name) => Deno.core.ops.op_pi_unregister_provider(name),
    setModel: (model) => Deno.core.ops.op_pi_set_model(model.id ?? model),
    getThinkingLevel: () => { try { return Deno.core.ops.op_pi_get_thinking_level(); } catch { return "medium"; } },
    setThinkingLevel: (level) => Deno.core.ops.op_pi_set_thinking_level(level),
    getActiveTools: () => { try { return Deno.core.ops.op_pi_get_active_tools(); } catch { return []; } },
    getAllTools: () => { try { return Deno.core.ops.op_pi_get_all_tools(); } catch { return []; } },
    setActiveTools: (toolNames) => { try { Deno.core.ops.op_pi_set_active_tools(toolNames); } catch {} },
    sendMessage: (message, options) => Deno.core.ops.op_pi_send_message(message.customType ?? "", message.content ?? ""),
    sendUserMessage: (content, options) => Deno.core.ops.op_pi_send_user_message(typeof content === "string" ? content : content?.content ?? ""),
    appendEntry: (customType, data) => Deno.core.ops.op_pi_append_entry(customType, data ?? null),
    setSessionName: (name) => Deno.core.ops.op_pi_set_session_name(name),
    getSessionName: () => { try { return Deno.core.ops.op_pi_get_session_name(); } catch { return undefined; } },
    setLabel: (entryId, label) => Deno.core.ops.op_pi_set_label(entryId, label ?? null),
    navigateTree: (direction) => { try { Deno.core.ops.op_pi_navigate_tree(direction); } catch {} },
    registerMessageRenderer: notSupported("registerMessageRenderer"),
    registerEntryRenderer: notSupported("registerEntryRenderer"),
  };
}

globalThis.__piMakePi = makePi;

// Rust calls this to (re)initialize registries before a load.
globalThis.__piClearRegistries = () => {
  tools.clear(); commands.clear(); flags.clear(); flagValues.clear();
  shortcuts.clear(); handlers.clear(); pendingNotifications.length = 0;
};

// Rust registers a tool's metadata mirror here (the handler stays in JS).
globalThis.__piRegisterTool = (tool) => { tools.set(tool.name, tool); };
globalThis.__piRegisterCommand = (name, opts) => { commands.set(name, opts); };
globalThis.__piRegisterShortcut = (key, opts) => { shortcuts.set(key, opts); };
globalThis.__piRegisterFlag = (name, opts) => {
  flags.set(name, opts);
  if (opts && opts.default !== undefined && !flagValues.has(name)) {
    flagValues.set(name, opts.default);
  }
};
globalThis.__piSetFlagValue = (name, value) => { flagValues.set(name, value); };
globalThis.__piGetFlags = () => Object.fromEntries(flagValues);

// Load a single extension: dynamic-import its module, call its default factory.
globalThis.__piLoadExtension = async (specifier) => {
  const mod = await import(specifier);
  const factory = mod.default ?? mod;
  if (typeof factory !== "function") {
    throw new Error(`Extension does not export a default factory: ${specifier}`);
  }
  const pi = makePi();
  await factory(pi);
};

// Helper to log and emit an extension error.
function emitExtensionError(eventType, error) {
  const msg = String(error && error.stack || error);
  try { Deno.core.ops.op_pi_log(msg); } catch {}
  try { Deno.core.ops.op_pi_emit_error("runtime.js", eventType, msg); } catch {}
}

// Fire-and-forget dispatch: serial await, results ignored.
globalThis.__piDispatch = async (eventType, payload) => {
  const ctx = makeContext(payload && payload.cwd);
  const list = handlers.get(eventType) ?? [];
  for (const h of list) {
    try { await h(payload, ctx); } catch (e) {
      emitExtensionError(eventType, e);
    }
  }
  return null;
};

// Result-returning dispatch with per-event aggregation (mirrors runner.ts).
globalThis.__piDispatchResult = async (eventType, payload) => {
  const ctx = makeContext(payload && payload.cwd);
  const list = handlers.get(eventType) ?? [];

  if (eventType === "tool_call") {
    for (const h of list) {
      try {
        const r = await h(payload, ctx);
        if (r && r.block) return { block: true, reason: r.reason };
      } catch (e) {
        emitExtensionError(eventType, e);
      }
    }
    return { block: false };
  }

  if (eventType === "tool_result") {
    let cur = {
      content: payload.content,
      details: payload.details,
      isError: payload.isError,
    };
    let modified = false;
    for (const h of list) {
      try {
        // Pass the accumulated `cur` (not the original payload) so later
        // handlers in a multi-extension chain see prior handlers' modifications
        // -- mirrors runner.ts and the message_end/context/before_provider_request
        // branches below.
        const r = await h({ ...payload, content: cur.content, details: cur.details, isError: cur.isError }, ctx);
        if (!r) continue;
        if (r.content !== undefined) { cur.content = r.content; modified = true; }
        if (r.details !== undefined) { cur.details = r.details; modified = true; }
        if (r.isError !== undefined) { cur.isError = r.isError; modified = true; }
      } catch (e) {
        emitExtensionError(eventType, e);
      }
    }
    return modified ? cur : null;
  }

  if (eventType === "message_end") {
    let cur = payload.message;
    let modified = false;
    for (const h of list) {
      try {
        const r = await h({ ...payload, message: cur }, ctx);
        if (r && r.message && r.message.role === cur.role) {
          cur = r.message; modified = true;
        }
      } catch (e) {
        emitExtensionError(eventType, e);
      }
    }
    return modified ? { message: cur } : null;
  }

  if (eventType === "context") {
    let cur = payload.messages;
    for (const h of list) {
      try {
        const r = await h({ type: "context", messages: cur }, ctx);
        if (r && r.messages) cur = r.messages;
      } catch (e) {
        emitExtensionError(eventType, e);
      }
    }
    return { messages: cur };
  }

  if (eventType === "before_provider_request") {
    let cur = payload.payload;
    for (const h of list) {
      try {
        const r = await h({ type: "before_provider_request", payload: cur }, ctx);
        if (r !== undefined) cur = r;
      } catch (e) {
        emitExtensionError(eventType, e);
      }
    }
    return { payload: cur };
  }

  if (eventType === "user_bash") {
    for (const h of list) {
      try {
        const r = await h(payload, ctx);
        if (r !== undefined) return r;
      } catch (e) {
        emitExtensionError(eventType, e);
      }
    }
    return null;
  }

  if (eventType === "resources_discover") {
    const result = { skillPaths: [], promptPaths: [], themePaths: [] };
    for (const h of list) {
      try {
        const r = await h(payload, ctx);
        if (!r) continue;
        if (r.skillPaths) result.skillPaths.push(...r.skillPaths);
        if (r.promptPaths) result.promptPaths.push(...r.promptPaths);
        if (r.themePaths) result.themePaths.push(...r.themePaths);
      } catch (e) {
        emitExtensionError(eventType, e);
      }
    }
    return result;
  }

  if (eventType === "project_trust") {
    for (const h of list) {
      try {
        const r = await h(payload, ctx);
        if (r && (r.trusted === "yes" || r.trusted === "no")) {
          return { trusted: r.trusted, remember: r.remember === true };
        }
      } catch (e) {
        emitExtensionError(eventType, e);
      }
    }
    return null;
  }

  if (eventType === "input") {
    let curText = payload.text;
    let curImages = payload.images;
    for (const h of list) {
      try {
        const r = await h({ type: "input", text: curText, images: curImages, source: payload.source }, ctx);
        if (!r) continue;
        if (r.action === "handled") return { action: "handled" };
        if (r.action === "transform") {
          if (r.text !== undefined) curText = r.text;
          if (r.images !== undefined) curImages = r.images;
        }
      } catch (e) {
        emitExtensionError(eventType, e);
      }
    }
    return { action: "continue", text: curText, images: curImages };
  }

  // Default: fire-and-forget semantics.
  for (const h of list) {
    try { await h(payload, ctx); } catch (e) {
      try { Deno.core.ops.op_pi_log(String(e && e.stack || e)); } catch {}
    }
  }
  return null;
};

// Tool execution: Rust calls this to invoke a registered tool's handler.
globalThis.__piCallTool = async (toolName, args, cwd) => {
  const entry = tools.get(toolName);
  if (!entry) throw new Error(`Tool not found: ${toolName}`);
  pendingNotifications.length = 0;
  const ctx = makeContext(cwd);
  const result = await entry.execute("", args, undefined, undefined, ctx);
  return { result, notifications: [...pendingNotifications] };
};

// Snapshot of registered tool metadata for Rust to build AgentTools from.
globalThis.__piGetToolInfos = () => {
  const out = [];
  for (const [name, t] of tools) {
    out.push({
      name,
      description: t.description ?? "",
      parameters: t.parameters ?? null,
      prompt_guidelines: t.promptGuidelines ?? null,
      execution_mode: t.executionMode ?? null,
    });
  }
  return out;
};

globalThis.__piGetCommands = () => {
  const out = [];
  for (const [name, c] of commands) {
    out.push({ name, description: c.description ?? null });
  }
  return out;
};