// The app side of the plugin bridge. Each enabled plugin runs in a sandboxed iframe
// (no same-origin, no network of its own); this relays its postMessage calls to the Rust host,
// which checks the plugin's permissions, and calls back into it for book sources and actions.
//
// Messages (all carry xs: 1):
//   frame → app  {kind: "hello"} | {kind: "call", id, method, params} | {kind: "reply", id, ok, value|error} | {kind: "log", …}
//   app → frame  {kind: "event", name, data} | {kind: "result", id, ok, value|error} | {kind: "invoke", id, handler, args}

export const SCHEME = "xsplugin";

/** URL of a path served by the plugin scheme (Tauri serves custom schemes differently on Windows). */
export function schemeUrl(path) {
  const windows = typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent);
  return windows ? `http://${SCHEME}.localhost/${path}` : `${SCHEME}://localhost/${path}`;
}

/** URL of a file inside a plugin. */
export function pluginUrl(id, path) {
  return schemeUrl(`${id}/${path.split("/").map(encodeURIComponent).join("/")}`);
}

/**
 * Why a plugin page may not have started: can the app fetch its page and the SDK over the
 * plugin scheme? Returns lines for the user.
 */
export async function diagnose(manifest, fetchImpl = fetch) {
  const probe = async (label, url) => {
    try {
      const r = await fetchImpl(url, { cache: "no-store" });
      const csp = r.headers?.get?.("content-security-policy");
      return `${label}: HTTP ${r.status}${r.headers?.get?.("content-type") ? `, ${r.headers.get("content-type")}` : ""}${csp ? ", CSP set" : ""}`;
    } catch (e) {
      return `${label}: failed — ${e?.message ?? e}`;
    }
  };
  return [
    await probe("Page", pluginUrl(manifest.id, manifest.entry ?? "index.html")),
    await probe("SDK", schemeUrl("_sdk/xstudio.js")),
  ];
}

const THEME_VARS = ["--paper", "--paper-2", "--sheet", "--ink", "--ink-2", "--ink-3", "--rule", "--rule-soft", "--rule-strong", "--on-accent", "--ok", "--danger", "--serif"];

export function currentTheme(doc = document) {
  const cs = getComputedStyle(doc.documentElement);
  const vars = {};
  for (const v of THEME_VARS) {
    const value = cs.getPropertyValue(v).trim();
    if (value) vars[v] = value;
  }
  const dark = typeof matchMedia === "function" && matchMedia("(prefers-color-scheme: dark)").matches;
  return { vars, dark };
}

export function toBase64(bytes) {
  if (bytes instanceof ArrayBuffer) bytes = new Uint8Array(bytes);
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

export function fromBase64(b64) {
  const s = atob(b64);
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
  return out;
}

/**
 * A plain copy of `v` that postMessage can clone. App state (Svelte 5 $state) is made of
 * Proxies, which structured clone rejects with DataCloneError; binary data is kept as is.
 */
export function plain(v, seen = new Map()) {
  if (v === null || typeof v !== "object") return typeof v === "function" ? undefined : v;
  if (ArrayBuffer.isView(v) || v instanceof ArrayBuffer || v instanceof Date || v instanceof Blob) return v;
  if (seen.has(v)) return seen.get(v);
  if (Array.isArray(v)) {
    const out = [];
    seen.set(v, out);
    for (const x of v) out.push(plain(x, seen));
    return out;
  }
  const out = {};
  seen.set(v, out);
  for (const [k, x] of Object.entries(v)) {
    const c = plain(x, seen);
    if (c !== undefined) out[k] = c;
  }
  return out;
}

const INVOKE_TIMEOUT = 10 * 60 * 1000; // downloads of large books take a while

export class PluginHost {
  /**
   * deps: {
   *   invoke(cmd, args) → Promise            Tauri invoke
   *   toast(message, kind, plugin)           show a message
   *   confirm(message, opts, plugin) → bool  ask the user
   *   saveDialog(name) → path | null
   *   bookNames() → [string]                 book files on the reader
   *   onSent()                               a book reached the reader
   *   log(level, message, plugin)
   * }
   */
  constructor(deps) {
    this.deps = deps;
    this.plugins = new Map(); // id → manifest
    this.frames = new Map(); // id → {win, ready, waiters: []}
    this.views = new Map(); // id → "section" | "settings"
    this.invokes = new Map(); // invoke id → {resolve, reject, timer}
    this.seq = 0;
    this.onMessage = (ev) => this.handle(ev);
  }

  start(target = window) {
    this.target = target;
    target.addEventListener("message", this.onMessage);
  }

  stop() {
    this.target?.removeEventListener("message", this.onMessage);
    for (const p of this.invokes.values()) {
      clearTimeout(p.timer);
      p.reject(new Error("closed"));
    }
    this.invokes.clear();
  }

  setPlugins(list) {
    this.plugins = new Map(list.map((m) => [m.id, plain(m)]));
    for (const id of [...this.frames.keys()]) if (!this.plugins.has(id)) this.frames.delete(id);
  }

  /** The iframe element for plugin `id` was created (or reloaded). */
  attach(id, iframe) {
    const prev = this.frames.get(id);
    this.frames.set(id, { iframe, win: iframe.contentWindow, ready: false, waiters: prev?.waiters ?? [] });
  }

  detach(id) {
    this.frames.delete(id);
  }

  setView(id, view) {
    this.views.set(id, view);
    this.event(id, "view", view);
  }

  broadcastTheme(theme) {
    for (const id of this.frames.keys()) this.event(id, "theme", theme);
  }

  event(id, name, data) {
    this.post(id, { kind: "event", name, data });
  }

  post(id, msg) {
    const f = this.frames.get(id);
    // A sandboxed frame has an opaque origin, so "*" is the only target that reaches it;
    // what we send is meant for that frame anyway.
    f?.win?.postMessage(plain({ xs: 1, ...msg }), "*");
  }

  /** Which plugin a message came from: only the frames we created count. */
  sourceOf(win) {
    for (const [id, f] of this.frames) {
      if (f.win && f.win === win) return id;
      if (f.iframe?.contentWindow === win) {
        f.win = win;
        return id;
      }
    }
    return null;
  }

  /** Resolves once the plugin page has loaded its SDK. */
  whenReady(id) {
    const f = this.frames.get(id);
    if (!f) return Promise.reject(new Error(`${this.plugins.get(id)?.name ?? id} is not running`));
    if (f.ready) return Promise.resolve();
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`${this.plugins.get(id)?.name ?? id} did not start`)), 15000);
      f.waiters.push(() => {
        clearTimeout(timer);
        resolve();
      });
    });
  }

  /** Calls something the plugin registered: "action.<id>". */
  async invoke(id, handler, ...args) {
    await this.whenReady(id);
    const callId = `i${++this.seq}`;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.invokes.delete(callId);
        reject(new Error(`${this.plugins.get(id)?.name ?? id} did not answer`));
      }, INVOKE_TIMEOUT);
      this.invokes.set(callId, { resolve, reject, timer, id });
      this.post(id, { kind: "invoke", id: callId, handler, args });
    });
  }

  async handle(ev) {
    const m = ev.data;
    if (!m || m.xs !== 1) return;
    const id = this.sourceOf(ev.source);
    if (!id) return; // not one of our plugin frames
    const manifest = this.plugins.get(id);
    if (!manifest) return;

    switch (m.kind) {
      case "hello": {
        const f = this.frames.get(id);
        this.event(id, "init", { manifest, view: this.views.get(id) ?? "section", theme: this.deps.theme?.() ?? null, apiVersion: 1 });
        f.ready = true;
        this.deps.onReady?.(id);
        const waiters = f.waiters.splice(0);
        waiters.forEach((w) => w());
        return;
      }
      case "reply": {
        const p = this.invokes.get(m.id);
        if (!p || p.id !== id) return;
        this.invokes.delete(m.id);
        clearTimeout(p.timer);
        m.ok ? p.resolve(m.value) : p.reject(new Error(m.error));
        return;
      }
      case "log":
        this.deps.log?.(m.level, String(m.message), manifest);
        return;
      case "call": {
        let reply;
        try {
          reply = { ok: true, value: await this.call(id, manifest, String(m.method), m.params ?? {}) };
        } catch (e) {
          reply = { ok: false, error: String(e?.message ?? e) };
        }
        this.post(id, { kind: "result", id: m.id, ...reply });
        return;
      }
    }
  }

  /** Methods the app answers itself; everything else goes to the Rust host. */
  async call(id, manifest, method, params) {
    const d = this.deps;
    switch (method) {
      case "ui.toast":
        d.toast(String(params.message ?? "").slice(0, 500), params.kind === "error" ? "error" : "info", manifest);
        return null;
      case "ui.confirm":
        return !!(await d.confirm(String(params.message ?? "").slice(0, 1000), { okLabel: params.okLabel, cancelLabel: params.cancelLabel }, manifest));
      case "device.books":
        if (!manifest.permissions?.device?.includes("read")) throw new Error('the plugin has no "device: read" permission');
        return d.bookNames();
      case "files.saveAs": {
        const path = await d.saveDialog(String(params.name ?? "file"));
        if (!path) return false;
        await d.invoke("plugin_save_file", { path, data: String(params.data ?? "") });
        return true;
      }
      default: {
        const value = await d.invoke("plugin_call", { id, method, params });
        if (method === "device.sendBook") d.onSent?.();
        return value;
      }
    }
  }
}

/** Book actions that apply to a file: [{plugin, action}]. */
export function bookActionsFor(plugins, fileName) {
  const ext = (fileName.match(/\.([a-z0-9]+)$/i)?.[1] ?? "").toLowerCase();
  const out = [];
  for (const p of plugins) {
    if (!p.enabled) continue;
    for (const action of p.manifest.contributes?.bookActions ?? []) {
      const formats = (action.formats ?? []).map((f) => f.toLowerCase());
      if (!formats.length || formats.includes(ext)) out.push({ plugin: p.manifest, action });
    }
  }
  return out;
}
