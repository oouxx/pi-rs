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

function makeContext(cwd) {
  const ctxCwd = cwd ?? sessionCwd ?? "/";
  const notSupported = (name) => () => {
    throw new Error(`ctx.${name} is not yet supported by the embedded runtime`);
  };
  return {
    cwd: ctxCwd,
    mode: "rpc",
    hasUI: false,
    ui: {
      // Notifications are buffered JS-side so __piCallTool can return them; the
      // Rust op_pi_notify mirrors into OpState for any Rust-side consumer too.
      notify: (msg, type) => {
        pendingNotifications.push(msg);
        try { Deno.core.ops.op_pi_notify(msg, type); } catch {}
      },
      setStatus: () => {},
    },
    exec: (command, args, options) =>
      execWithDefaultCwd(command, args, options, ctxCwd),
    // Deferred context members (not yet supported in phase 1) -- no-ops/stubs.
    isIdle: () => true,
    isProjectTrusted: () => true,
    abort: () => {},
    hasPendingMessages: () => false,
    shutdown: () => {},
    getSystemPrompt: () => "",
    // ExtensionCommandContext methods (Phase 4.3) -- stubs for now.
    waitForIdle: notSupported("waitForIdle"),
    newSession: notSupported("newSession"),
    fork: notSupported("fork"),
    navigateTree: notSupported("navigateTree"),
    switchSession: notSupported("switchSession"),
    reload: notSupported("reload"),
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
    events: { emit: () => {}, on: () => () => {} },
    // Deferred API -- present so extensions import cleanly, but throw on use.
    registerProvider: notSupported("registerProvider"),
    unregisterProvider: notSupported("unregisterProvider"),
    setModel: notSupported("setModel"),
    getThinkingLevel: () => "medium",
    setThinkingLevel: notSupported("setThinkingLevel"),
    getActiveTools: () => [],
    getAllTools: () => [],
    setActiveTools: notSupported("setActiveTools"),
    sendMessage: (message, options) => Deno.core.ops.op_pi_send_message(message.customType ?? "", message.content ?? ""),
    sendUserMessage: (content, options) => Deno.core.ops.op_pi_send_user_message(typeof content === "string" ? content : content?.content ?? ""),
    appendEntry: (customType, data) => Deno.core.ops.op_pi_append_entry(customType, data ?? null),
    setSessionName: notSupported("setSessionName"),
    getSessionName: () => undefined,
    setLabel: notSupported("setLabel"),
    navigateTree: notSupported("navigateTree"),
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

// Fire-and-forget dispatch: serial await, results ignored.
globalThis.__piDispatch = async (eventType, payload) => {
  const ctx = makeContext(payload && payload.cwd);
  const list = handlers.get(eventType) ?? [];
  for (const h of list) {
    try { await h(payload, ctx); } catch (e) {
      // Best-effort: log via op, don't break the dispatch chain.
      try { Deno.core.ops.op_pi_log(String(e && e.stack || e)); } catch {}
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
        try { Deno.core.ops.op_pi_log(String(e && e.stack || e)); } catch {}
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
        const r = await h(payload, ctx);
        if (!r) continue;
        if (r.content !== undefined) { cur.content = r.content; modified = true; }
        if (r.details !== undefined) { cur.details = r.details; modified = true; }
        if (r.isError !== undefined) { cur.isError = r.isError; modified = true; }
      } catch (e) {
        try { Deno.core.ops.op_pi_log(String(e && e.stack || e)); } catch {}
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
        try { Deno.core.ops.op_pi_log(String(e && e.stack || e)); } catch {}
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
        try { Deno.core.ops.op_pi_log(String(e && e.stack || e)); } catch {}
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
        try { Deno.core.ops.op_pi_log(String(e && e.stack || e)); } catch {}
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
        try { Deno.core.ops.op_pi_log(String(e && e.stack || e)); } catch {}
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
        try { Deno.core.ops.op_pi_log(String(e && e.stack || e)); } catch {}
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
        try { Deno.core.ops.op_pi_log(String(e && e.stack || e)); } catch {}
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
        try { Deno.core.ops.op_pi_log(String(e && e.stack || e)); } catch {}
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