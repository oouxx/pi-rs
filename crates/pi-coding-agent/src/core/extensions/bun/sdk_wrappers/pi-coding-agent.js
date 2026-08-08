// @earendil-works/pi-coding-agent — 真实 SDK bundle + RPC 桥接。
// bundle.js 是 TS 源码转译产物（xtask build-sdk）；工具工厂经
// globalThis.__pi.__runBuiltinTool RPC 到宿主跑真实内置工具。
export * from "./bundle.js";

export const VERSION = "0.84.0";
export const CONFIG_DIR_NAME = ".pi-rs";

export function defineTool(def) { return def; }

export function getAgentDir() {
  return process.env.PI_RS_HOME
    ? process.env.PI_RS_HOME + "/agent"
    : require("os").homedir() + "/.pi-rs/agent";
}

function builtinToolFactory(name, description, parameters) {
  return {
    name,
    label: name,
    description,
    parameters,
    execute: async (toolCallId, params) => {
      return globalThis.__pi.__runBuiltinTool(name, params);
    },
  };
}

export function createBashTool() {
  return builtinToolFactory(
    "bash",
    "Run a bash command in the project directory",
    { type: "object", properties: { command: { type: "string", description: "The bash command to run" } }, required: ["command"] },
  );
}

export function createReadTool() {
  return builtinToolFactory(
    "read",
    "Read a file from the filesystem",
    { type: "object", properties: { path: { type: "string", description: "Path to the file to read" } }, required: ["path"] },
  );
}

export function createWriteTool() {
  return builtinToolFactory(
    "write",
    "Write a file to the filesystem",
    { type: "object", properties: { path: { type: "string" }, content: { type: "string" } }, required: ["path", "content"] },
  );
}

export function createEditTool() {
  return builtinToolFactory(
    "edit",
    "Edit a file using exact text replacement",
    { type: "object", properties: { path: { type: "string" }, edits: { type: "array" } }, required: ["path", "edits"] },
  );
}
