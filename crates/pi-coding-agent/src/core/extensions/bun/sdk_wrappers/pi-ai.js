// @earendil-works/pi-ai — 真实 SDK bundle + RPC 桥接。
// bundle.js 是 TS 源码转译产物（xtask build-sdk）；complete/getModel/
// registerApiProvider 经 globalThis.__pi RPC 到宿主。
export * from "./bundle.js";

export async function complete(model, context, options) {
  return globalThis.__pi.__piAiComplete({
    model: { provider: model?.provider, id: model?.id },
    context,
    options: options || null,
  });
}

export function streamSimple(model, context, options) {
  return { async result() { return complete(model, context, options); } };
}

export async function completeSimple(model, context, options) {
  return complete(model, context, options);
}

export function getModel() {
  return globalThis.__pi.__piGetModel();
}

export function registerApiProvider(name, config) {
  return globalThis.__pi.__piRegisterProvider(name, config);
}
