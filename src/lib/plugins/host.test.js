import { describe, it, expect, vi } from "vitest";
import { PluginHost, bookActionsFor, pluginUrl, toBase64, fromBase64 } from "./host.js";

const manifest = {
  id: "dev.test.p",
  name: "Test",
  permissions: { device: ["write"] },
  contributes: { section: { title: "Test" }, bookActions: [{ id: "fix", title: "Fix", formats: ["epub"] }] },
};

/** A fake plugin frame: records what the app posts to it. */
function frame() {
  // structuredClone: what a real postMessage does, so un-cloneable data fails here too.
  const win = { sent: [], postMessage(msg) { this.sent.push(structuredClone(msg)); } };
  return { win, iframe: { contentWindow: win } };
}

function setup(deps = {}) {
  const invoke = vi.fn(async (cmd, args) => ({ cmd, args }));
  const host = new PluginHost({
    invoke,
    toast: vi.fn(),
    confirm: vi.fn(async () => true),
    saveDialog: vi.fn(async () => "/tmp/out.epub"),
    bookNames: () => ["a.epub"],
    onSent: vi.fn(),
    theme: () => ({ vars: {}, dark: false }),
    ...deps,
  });
  host.setPlugins([manifest]);
  const f = frame();
  host.attach(manifest.id, f.iframe);
  return { host, f, invoke };
}

const msg = (source, data) => ({ source, data: { xs: 1, ...data } });

describe("PluginHost", () => {
  it("answers hello with init and only listens to its own frames", async () => {
    const { host, f, invoke } = setup();
    await host.handle(msg({ postMessage() {} }, { kind: "call", id: 1, method: "storage.get", params: {} }));
    expect(invoke).not.toHaveBeenCalled();

    await host.handle(msg(f.win, { kind: "hello" }));
    expect(f.win.sent[0]).toMatchObject({ kind: "event", name: "init", data: { manifest, view: "section" } });
  });

  it("relays calls to Rust with the plugin's own id", async () => {
    const { host, f, invoke } = setup();
    await host.handle(msg(f.win, { kind: "call", id: 7, method: "http.fetch", params: { url: "https://x" }, pluginId: "dev.other" }));
    expect(invoke).toHaveBeenCalledWith("plugin_call", { id: "dev.test.p", method: "http.fetch", params: { url: "https://x" } });
    expect(f.win.sent.at(-1)).toMatchObject({ kind: "result", id: 7, ok: true });
  });

  it("handles UI methods itself and checks device read", async () => {
    const { host, f, invoke } = setup();
    await host.handle(msg(f.win, { kind: "call", id: 1, method: "ui.confirm", params: { message: "Sure?" } }));
    expect(f.win.sent.at(-1)).toMatchObject({ ok: true, value: true });
    await host.handle(msg(f.win, { kind: "call", id: 2, method: "device.books" }));
    expect(f.win.sent.at(-1)).toMatchObject({ ok: false });
    expect(f.win.sent.at(-1).error).toContain("device: read");
    await host.handle(msg(f.win, { kind: "call", id: 3, method: "files.saveAs", params: { name: "a.epub", data: "AA==" } }));
    expect(invoke).toHaveBeenCalledWith("plugin_save_file", { path: "/tmp/out.epub", data: "AA==" });
  });

  it("reports errors from Rust back to the plugin", async () => {
    const { host, f } = setup({ invoke: async () => { throw "the plugin may not connect to evil.com"; } });
    await host.handle(msg(f.win, { kind: "call", id: 9, method: "http.fetch", params: {} }));
    expect(f.win.sent.at(-1)).toEqual({ xs: 1, kind: "result", id: 9, ok: false, error: "the plugin may not connect to evil.com" });
  });

  it("invokes registered handlers once the frame is ready", async () => {
    const { host, f } = setup();
    const result = host.invoke(manifest.id, "action.fix", "мастер", null);
    await host.handle(msg(f.win, { kind: "hello" }));
    await Promise.resolve();
    const inv = f.win.sent.find((m) => m.kind === "invoke");
    expect(inv).toMatchObject({ handler: "action.fix", args: ["мастер", null] });
    // A reply from another frame is ignored.
    await host.handle(msg({}, { kind: "reply", id: inv.id, ok: true, value: "spoofed" }));
    await host.handle(msg(f.win, { kind: "reply", id: inv.id, ok: true, value: { books: [] } }));
    await expect(result).resolves.toEqual({ books: [] });
  });
});

describe("app state", () => {
  it("sends Proxy-wrapped state (Svelte $state) to the frame", async () => {
    // Like Svelte's $state: plain objects and arrays become Proxies, typed arrays don't.
    const proxied = (v) => v !== null && typeof v === "object" && (Array.isArray(v) || Object.getPrototypeOf(v) === Object.prototype);
    const deep = (o) => new Proxy(o, { get: (t, k) => (proxied(t[k]) ? deep(t[k]) : t[k]) });
    expect(() => structuredClone(deep({ a: 1 }))).toThrow(); // what broke the plugin
    const { host, f } = setup();
    host.setPlugins([deep({ ...manifest })]);
    host.attach(manifest.id, f.iframe);
    await host.handle(msg(f.win, { kind: "hello" }));
    expect(f.win.sent[0].data.manifest.contributes.section.title).toBe("Test");

    const book = deep({ id: "b1", title: "Книга", authors: ["А"], data: new Uint8Array([1, 2]) });
    const done = host.invoke(manifest.id, "action.fix", book);
    await Promise.resolve();
    const inv = f.win.sent.find((m) => m.kind === "invoke");
    expect(inv.args[0]).toMatchObject({ id: "b1", authors: ["А"] });
    expect(inv.args[0].data).toEqual(new Uint8Array([1, 2]));
    await host.handle(msg(f.win, { kind: "reply", id: inv.id, ok: true, value: null }));
    await done;
  });
});

describe("helpers", () => {
  it("builds plugin URLs", () => {
    expect(pluginUrl("dev.a.b", "ui/index.html")).toBe("xsplugin://localhost/dev.a.b/ui/index.html");
  });

  it("finds book actions for a file", () => {
    const plugins = [{ enabled: true, manifest }, { enabled: false, manifest: { ...manifest, id: "off" } }];
    expect(bookActionsFor(plugins, "Book.EPUB")).toHaveLength(1);
    expect(bookActionsFor(plugins, "book.fb2")).toHaveLength(0);
  });

  it("round-trips base64", () => {
    const bytes = new Uint8Array(70000).map((_, i) => i % 256);
    expect(fromBase64(toBase64(bytes))).toEqual(bytes);
  });
});
