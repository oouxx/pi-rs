/**
 * pi-rpc-host — Bun subprocess for executing pi extensions via JSON-RPC.
 *
 * Protocol: line-delimited JSON-RPC 2.0 over stdin/stdout.
 * Each line is a complete JSON object terminated by \n.
 *
 * Supported methods:
 *   load       — Load extensions, return registered tools/commands
 *   call_tool  — Execute an extension tool handler by name
 *   reload     — Clear cache and reload all extensions
 *   shutdown   — Graceful exit
 */

import * as fs from "node:fs";
import * as path from "node:path";
import { spawn, type ChildProcess } from "node:child_process";

// ============================================================================
// JSON-RPC Types
// ============================================================================

interface RpcRequest {
  jsonrpc: "2.0";
  id: number;
  method: string;
  params?: Record<string, unknown>;
}

interface RpcSuccess {
  jsonrpc: "2.0";
  id: number;
  result: unknown;
}

interface RpcError {
  jsonrpc: "2.0";
  id: number;
  error: { code: number; message: string; data?: unknown };
}

interface RpcNotification {
  jsonrpc: "2.0";
  method: string;
  params?: Record<string, unknown>;
}

// ============================================================================
// Extension Types (simplified from original pi-coding-agent)
// ============================================================================

interface ToolDefinition {
  name: string;
  description: string;
  parameters?: Record<string, unknown>;
  prompt_guidelines?: string[];
  execution_mode?: "sequential" | "parallel" | "readonly";
  /** The actual handler function (kept in Bun process) */
  handler?: (args: Record<string, unknown>, ctx: ExtensionContext) => Promise<unknown>;
}

interface CommandDefinition {
  name: string;
  description?: string;
  handler?: (args: string, ctx: ExtensionContext) => Promise<void>;
}

interface ShortcutDefinition {
  key: string;
  description?: string;
  handler?: (ctx: ExtensionContext) => Promise<void>;
}

interface FlagDefinition {
  name: string;
  description?: string;
  type: "boolean" | "string";
  default?: boolean | string;
}

interface ExecResult {
  stdout: string;
  stderr: string;
  exitCode: number;
}

interface ExecOptions {
  cwd?: string;
  timeout?: number;
}

interface UiContext {
  notify(message: string, type?: "info" | "warning" | "error"): void;
  setStatus(key: string, text: string | undefined): void;
}

interface ExtensionContext {
  cwd: string;
  ui: UiContext;
  exec: (command: string, args?: string[], options?: ExecOptions) => Promise<ExecResult>;
  notify?: (message: string) => void;
}

// Queued notifications from handlers (sent back with response)
const pendingNotifications: string[] = [];

const uiContext: UiContext = {
  notify(message, type) {
    pendingNotifications.push(`[${type ?? "info"}] ${message}`);
  },
  setStatus(_key, _text) {
    // Status updates are tracked locally; not sent back yet
  },
};

// ============================================================================
// Extension API — matches what pi extensions expect
// ============================================================================

interface ExtensionApi {
  registerTool(tool: ToolDefinition): void;
  registerCommand(name: string, options: Partial<CommandDefinition> & { description?: string }): void;
  on(event: string, handler: (...args: unknown[]) => unknown | Promise<unknown>): void;
  registerShortcut(shortcut: string, options: { description?: string; handler: (ctx: ExtensionContext) => Promise<void> }): void;
  registerFlag(name: string, options: FlagDefinition): void;
  exec: (command: string, args?: string[], options?: ExecOptions) => Promise<ExecResult>;
  getFlag(name: string): boolean | string | undefined;
  getCommands(): Array<{ name: string; description?: string; source: string }>;
}

// ============================================================================
// Extension Registry (holds loaded extension state)
// ============================================================================

interface LoadedExtension {
  path: string;
  resolvedPath: string;
  tools: Map<string, ToolDefinition>;
  commands: Map<string, CommandDefinition>;
  shortcuts: Map<string, ShortcutDefinition>;
  flags: Map<string, FlagDefinition>;
  handlers: Map<string, Array<(...args: unknown[]) => unknown | Promise<unknown>>>;
  flagValues: Map<string, boolean | string>;
}

const extensionRegistry = {
  extensions: new Map<string, LoadedExtension>(),
  allTools: new Map<string, { tool: ToolDefinition; extensionPath: string }>(),
  allCommands: new Map<string, { cmd: CommandDefinition; extensionPath: string }>(),
  flagValues: new Map<string, boolean | string>(),

  clear() {
    this.extensions.clear();
    this.allTools.clear();
    this.allCommands.clear();
    this.flagValues.clear();
  },

  registerTool(name: string, tool: ToolDefinition, extPath: string) {
    this.allTools.set(name, { tool, extensionPath: extPath });
  },

  registerCommand(name: string, cmd: CommandDefinition, extPath: string) {
    this.allCommands.set(name, { cmd, extensionPath: extPath });
  },

  getTool(name: string) {
    return this.allTools.get(name);
  },

  getCommand(name: string) {
    return this.allCommands.get(name);
  },
};

// ============================================================================
// Extension Loader
// ============================================================================

let jitiInstance: any = null;

async function getJiti(): Promise<any> {
  if (jitiInstance) return jitiInstance;
  // Use dynamic import for jiti (it's an ESM-only package)
  const { createJiti } = await import("jiti");
  jitiInstance = createJiti(import.meta.url, {
    moduleCache: false,
    // In Bun, native import handles TS; jiti provides virtual modules for @earendil-works/pi-*
    virtualModules: {
      "@earendil-works/pi-coding-agent": `export default {}`,
      "@earendil-works/pi-agent-core": `export default {}`,
      "@earendil-works/pi-ai": `export default {}`,
      "@earendil-works/pi-tui": `export default {}`,
      "@mariozechner/pi-coding-agent": `export default {}`,
      "@mariozechner/pi-agent-core": `export default {}`,
      "@mariozechner/pi-ai": `export default {}`,
      "@mariozechner/pi-tui": `export default {}`,
    },
  });
  return jitiInstance;
}

function createExtensionApi(extension: LoadedExtension): ExtensionApi {
  const api: ExtensionApi = {
    registerTool(tool: ToolDefinition) {
      extension.tools.set(tool.name, tool);
      extensionRegistry.registerTool(tool.name, tool, extension.path);
    },

    registerCommand(name: string, options: Partial<CommandDefinition> & { description?: string }) {
      const cmd: CommandDefinition = {
        name,
        description: options.description,
        handler: options.handler,
      };
      extension.commands.set(name, cmd);
      extensionRegistry.registerCommand(name, cmd, extension.path);
    },

    on(event: string, handler: (...args: unknown[]) => unknown | Promise<unknown>) {
      const list = extension.handlers.get(event) ?? [];
      list.push(handler);
      extension.handlers.set(event, list);
    },

    registerShortcut(shortcut: string, options: { description?: string; handler: (ctx: ExtensionContext) => Promise<void> }) {
      extension.shortcuts.set(shortcut, { key: shortcut, ...options });
    },

    registerFlag(name: string, options: FlagDefinition) {
      extension.flags.set(name, options);
      if (options.default !== undefined && !extension.flagValues.has(name)) {
        extension.flagValues.set(name, options.default);
        extensionRegistry.flagValues.set(name, options.default);
      }
    },

    exec: execCommand,
    getFlag(name: string) {
      return extension.flagValues.get(name) ?? extensionRegistry.flagValues.get(name);
    },
    getCommands() {
      return Array.from(extensionRegistry.allCommands.entries()).map(([name, { cmd }]) => ({
        name,
        description: cmd.description,
        source: "extension",
      }));
    },
  };

  return api;
}

// ============================================================================
// Command Execution (handles ctx.exec() from extensions)
// ============================================================================

async function execCommand(
  command: string,
  args: string[] = [],
  options?: ExecOptions,
): Promise<ExecResult> {
  return new Promise((resolve, reject) => {
    const child: ChildProcess = spawn(command, args, {
      cwd: options?.cwd,
      stdio: ["ignore", "pipe", "pipe"],
      timeout: options?.timeout ?? 30000,
    });

    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];

    child.stdout?.on("data", (data: Buffer) => stdout.push(data));
    child.stderr?.on("data", (data: Buffer) => stderr.push(data));

    child.on("close", (exitCode) => {
      resolve({
        stdout: Buffer.concat(stdout).toString("utf-8"),
        stderr: Buffer.concat(stderr).toString("utf-8"),
        exitCode: exitCode ?? -1,
      });
    });

    child.on("error", (err) => {
      reject(err);
    });
  });
}

// ============================================================================
// Extension Discovery
// ============================================================================

function discoverExtensions(extensionsDir: string): string[] {
  if (!fs.existsSync(extensionsDir)) return [];
  const entries = fs.readdirSync(extensionsDir, { withFileTypes: true });
  const discovered: string[] = [];

  for (const entry of entries) {
    const entryPath = path.join(extensionsDir, entry.name);

    // Direct .ts / .js files
    if (entry.isFile() && (entry.name.endsWith(".ts") || entry.name.endsWith(".js"))) {
      discovered.push(entryPath);
      continue;
    }

    // Subdirectories (follows symlinks on most platforms)
    if (entry.isDirectory() || entry.isSymbolicLink()) {
      // Check for pi manifest entries in package.json
      const pkgJsonPath = path.join(entryPath, "package.json");
      if (fs.existsSync(pkgJsonPath)) {
        try {
          const pkgContent = fs.readFileSync(pkgJsonPath, "utf-8");
          const pkg = JSON.parse(pkgContent);
          if (pkg.pi?.extensions?.length) {
            for (const extPath of pkg.pi.extensions) {
              const resolvedDir = path.resolve(entryPath, extPath);
              // If it's a directory, look for index.ts/index.js inside
              if (fs.existsSync(resolvedDir)) {
                const dirStat = fs.statSync(resolvedDir);
                if (dirStat.isDirectory() || dirStat.isFile()) {
                  if (dirStat.isFile()) {
                    discovered.push(resolvedDir);
                  } else {
                    for (const idxFile of ["index.ts", "index.js"]) {
                      const idxPath = path.join(resolvedDir, idxFile);
                      if (fs.existsSync(idxPath)) {
                        discovered.push(idxPath);
                        break;
                      }
                    }
                  }
                }
              }
            }
            continue; // skip the default index.ts check since pi manifest was found
          }
        } catch {
          // Invalid JSON, fall through to index.ts check
        }
      }
      // Fallback: look for index.ts/index.js in the entry root
      for (const indexFile of ["index.ts", "index.js"]) {
        const indexPath = path.join(entryPath, indexFile);
        if (fs.existsSync(indexPath)) {
          discovered.push(indexPath);
          break;
        }
      }
    }
  }

  return discovered;
}

// ============================================================================
// Extension Loading
// ============================================================================

async function loadSingleExtension(
  filePath: string,
  _cwd: string,
): Promise<{ extension: LoadedExtension | null; error: string | null }> {
  const resolvedPath = path.resolve(filePath);
  if (!fs.existsSync(resolvedPath)) {
    return { extension: null, error: `Extension file not found: ${filePath}` };
  }

  try {
    const jiti = await getJiti();
    const mod = await jiti.import(resolvedPath, { default: true });

    const factory = mod.default ?? mod;
    if (typeof factory !== "function") {
      return { extension: null, error: `Extension does not export a default factory function: ${filePath}` };
    }

    const extension: LoadedExtension = {
      path: filePath,
      resolvedPath,
      tools: new Map(),
      commands: new Map(),
      shortcuts: new Map(),
      flags: new Map(),
      handlers: new Map(),
      flagValues: new Map(),
    };

    const api = createExtensionApi(extension);
    // Provide a simple context for initialization (minimal)
    await factory(api);

    return { extension, error: null };
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return { extension: null, error: `Failed to load extension: ${message}` };
  }
}

interface LoadResult {
  tools: Array<{
    name: string;
    description: string;
    parameters?: Record<string, unknown>;
    prompt_guidelines?: string[];
    execution_mode?: string;
  }>;
  commands: Array<{
    name: string;
    description?: string;
  }>;
  errors: Array<{ path: string; error: string }>;
}

async function loadExtensions(
  cwd: string,
  agentDir: string,
  configuredPaths: string[],
): Promise<LoadResult> {
  extensionRegistry.clear();
  const errors: Array<{ path: string; error: string }> = [];
  const seen = new Set<string>();
  const allPaths: string[] = [];

  const addPath = (p: string) => {
    const resolved = path.resolve(p);
    if (!seen.has(resolved)) {
      seen.add(resolved);
      allPaths.push(p);
    }
  };

  // 1. Project-local extensions: {cwd}/.pi-rs/extensions/
  const projectExtDir = path.join(cwd, ".pi-rs", "extensions");
  for (const ext of discoverExtensions(projectExtDir)) {
    addPath(ext);
  }

  // 2. Global extensions: {agentDir}/extensions/
  const globalExtDir = path.join(agentDir, "extensions");
  for (const ext of discoverExtensions(globalExtDir)) {
    addPath(ext);
  }

  // 3. Explicitly configured paths
  for (const p of configuredPaths) {
    const resolved = path.resolve(p);
    const stat = fs.statSync(resolved);
    if (fs.existsSync(resolved) && (stat.isDirectory() || stat.isFile())) {
      // Check for index.ts or package.json in directory
      let found = false;
      if (stat.isFile()) {
        // Single file extension entry
        addPath(resolved);
        found = true;
      } else {
        for (const indexFile of ["index.ts", "index.js"]) {
          const indexPath = path.join(resolved, indexFile);
          if (fs.existsSync(indexPath)) {
            addPath(indexPath);
            found = true;
            break;
          }
        }
      }
      if (!found) {
        // Scan individual files
        for (const ext of discoverExtensions(resolved)) {
          addPath(ext);
        }
      }
    } else {
      addPath(resolved);
    }
  }

  // Load each extension
  for (const extPath of allPaths) {
    const { extension, error } = await loadSingleExtension(extPath, cwd);
    if (error) {
      errors.push({ path: extPath, error });
    } else if (extension) {
      extensionRegistry.extensions.set(extPath, extension);
    }
  }

  // Build result
  const tools = Array.from(extensionRegistry.allTools.values()).map(({ tool }) => ({
    name: tool.name,
    description: tool.description,
    parameters: tool.parameters,
    prompt_guidelines: tool.prompt_guidelines,
    execution_mode: tool.execution_mode,
  }));

  const commands = Array.from(extensionRegistry.allCommands.values()).map(({ cmd }) => ({
    name: cmd.name,
    description: cmd.description,
  }));

  return { tools, commands, errors };
}

// ============================================================================
// Tool Call Execution
// ============================================================================

async function callTool(
  toolName: string,
  args: Record<string, unknown>,
  cwd: string,
): Promise<{ result: unknown; notifications: string[] }> {
  const registered = extensionRegistry.getTool(toolName);
  if (!registered) {
    throw new Error(`Tool not found: ${toolName}`);
  }

  const { tool } = registered;
  if (!tool.handler || typeof tool.handler !== "function") {
    throw new Error(`Tool "${toolName}" has no executable handler`);
  }

  // Clear pending notifications from previous calls
  pendingNotifications.length = 0;

  const ctx: ExtensionContext = {
    cwd,
    ui: uiContext,
    exec: execCommand,
  };

  const result = await tool.handler(args, ctx);

  return {
    result,
    notifications: [...pendingNotifications],
  };
}

// ============================================================================
// JSON-RPC Server (stdin/stdout line-delimited)
// ============================================================================

import * as readline from "node:readline";
const rl = readline.createInterface({
  input: process.stdin,
  output: process.stdout,
  terminal: false,
});

let pendingLoadCwd = "/tmp";
let pendingLoadAgentDir = "";

function sendResponse(id: number, result: unknown): void {
  const msg: RpcSuccess = { jsonrpc: "2.0", id, result };
  process.stdout.write(JSON.stringify(msg) + "\n");
}

function sendError(id: number, code: number, message: string, data?: unknown): void {
  const msg: RpcError = { jsonrpc: "2.0", id, error: { code, message, data } };
  process.stdout.write(JSON.stringify(msg) + "\n");
}

function sendNotification(method: string, params?: Record<string, unknown>): void {
  const msg: RpcNotification = { jsonrpc: "2.0", method, params };
  process.stdout.write(JSON.stringify(msg) + "\n");
}

async function handleRequest(req: RpcRequest): Promise<void> {
  const { id, method, params } = req;

  try {
    switch (method) {
      case "load": {
        const { cwd, agentDir, extensionPaths } = (params ?? {}) as {
          cwd?: string;
          agentDir?: string;
          extensionPaths?: string[];
        };
        pendingLoadCwd = cwd ?? "/tmp";
        pendingLoadAgentDir = agentDir ?? "";
        const result = await loadExtensions(
          pendingLoadCwd,
          pendingLoadAgentDir,
          extensionPaths ?? [],
        );
        sendResponse(id, result);
        break;
      }

      case "call_tool": {
        const { toolName, toolArgs, cwd } = (params ?? {}) as {
          toolName?: string;
          toolArgs?: Record<string, unknown>;
          cwd?: string;
        };
        if (!toolName) {
          sendError(id, -32602, "Missing required parameter: toolName");
          break;
        }
        const { result, notifications } = await callTool(
          toolName,
          toolArgs ?? {},
          cwd ?? pendingLoadCwd,
        );
        sendResponse(id, { result, notifications });
        break;
      }

      case "reload": {
        const { cwd, agentDir, extensionPaths } = (params ?? {}) as {
          cwd?: string;
          agentDir?: string;
          extensionPaths?: string[];
        };
        pendingLoadCwd = cwd ?? pendingLoadCwd;
        pendingLoadAgentDir = agentDir ?? pendingLoadAgentDir;
        const result = await loadExtensions(
          pendingLoadCwd,
          pendingLoadAgentDir,
          extensionPaths ?? [],
        );
        sendResponse(id, result);
        break;
      }

      case "shutdown": {
        sendResponse(id, "ok");
        // Small delay to ensure response is sent before exit
        setTimeout(() => process.exit(0), 50);
        break;
      }

      case "ping": {
        sendResponse(id, "pong");
        break;
      }

      default: {
        sendError(id, -32601, `Method not found: ${method}`);
        break;
      }
    }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    sendError(id, -32603, `Internal error: ${message}`);
  }
}

// Sequential request queue — ensures responses are in order
let requestQueue: Promise<void> = Promise.resolve();

function enqueueRequest(request: RpcRequest): void {
  requestQueue = requestQueue.then(() =>
    handleRequest(request).catch((err) => {
      sendError(request.id, -32603, `Unhandled error: ${err.message}`);
    }),
  );
}

// Main loop: read lines from stdin
rl.on("line", (line: string) => {
  line = line.trim();
  if (!line) return;

  let request: RpcRequest;
  try {
    request = JSON.parse(line) as RpcRequest;
  } catch {
    sendNotification("log", { level: "error", message: `Invalid JSON: ${line.slice(0, 100)}` });
    return;
  }

  // Validate JSON-RPC
  if (request.jsonrpc !== "2.0" || !request.method) {
    return;
  }

  enqueueRequest(request);
});

rl.on("close", () => {
  process.exit(0);
});

// Handle SIGTERM from parent process
process.on("SIGTERM", () => {
  process.exit(0);
});

// Report ready
sendNotification("ready", { pid: process.pid, runtime: "bun" });
