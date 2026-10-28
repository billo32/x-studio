import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import vm from "node:vm";

// Installed plugins call whatever SDK the app ships, so nothing published may disappear.
// Add to this list when the API grows; never remove from it.
const API_V1 = [
  "ready", "on", "http.fetch", "storage.get", "storage.set", "storage.delete", "storage.keys",
  "secrets.set", "secrets.has", "secrets.delete", "auth.signIn", "device.sendBook", "device.books",
  "epub.prepare", "files.saveAs", "ui.toast", "ui.confirm", "registerBookAction",
  "registerBookSource", // retired, kept as a no-op
  "util.toBase64", "util.fromBase64",
];

function loadSdk() {
  const parent = { postMessage() {} };
  const window = { parent, addEventListener() {} };
  const context = vm.createContext({ window, console: { warn() {} }, btoa, atob, TextDecoder, Uint8Array, ArrayBuffer, Map, Set, Promise });
  vm.runInContext(readFileSync(new URL("../../../plugin-sdk/xstudio.js", import.meta.url), "utf8"), context);
  return window.xstudio;
}

describe("plugin SDK", () => {
  it("keeps every API v1 member", () => {
    const xs = loadSdk();
    const missing = API_V1.filter((path) => path.split(".").reduce((o, k) => o?.[k], xs) === undefined);
    expect(missing).toEqual([]);
    expect(() => xs.registerBookSource({ list() {} })).not.toThrow();
  });
});
