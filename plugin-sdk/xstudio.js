// X Studio plugin SDK (API v1). Load it first in your page:
//   <script src="/_sdk/xstudio.js"></script>
// It sets up `window.xstudio`. The page runs in a sandboxed frame with no network of its own:
// everything below goes to the app, which checks your manifest's permissions.
// Reference: PLUGINS.md in the X Studio repository.
(() => {
  "use strict";
  if (window.xstudio) return;

  const parent = window.parent;
  let seq = 0;
  const pending = new Map(); // call id → {resolve, reject}
  const handlers = new Map(); // "action.<id>" → fn
  const listeners = new Map(); // event name → Set<fn>
  let initData = null;
  let resolveReady;
  const ready = new Promise((r) => (resolveReady = r));

  function post(msg) {
    // The app's origin isn't known to an opaque-origin frame; the app checks event.source.
    parent.postMessage({ xs: 1, ...msg }, "*");
  }

  function call(method, params = {}) {
    const id = ++seq;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      post({ kind: "call", id, method, params });
    });
  }

  function emit(name, data) {
    for (const fn of listeners.get(name) ?? []) {
      try {
        fn(data);
      } catch (e) {
        console.error(e);
      }
    }
  }

  window.addEventListener("message", async (ev) => {
    if (ev.source !== parent || !ev.data || ev.data.xs !== 1) return;
    const m = ev.data;
    if (m.kind === "result") {
      const p = pending.get(m.id);
      if (!p) return;
      pending.delete(m.id);
      m.ok ? p.resolve(m.value) : p.reject(new Error(m.error));
    } else if (m.kind === "invoke") {
      // The app calls something the plugin registered (a book source, a book action).
      const fn = handlers.get(m.handler);
      try {
        if (!fn) throw new Error(`${m.handler} is not registered`);
        post({ kind: "reply", id: m.id, ok: true, value: await fn(...(m.args ?? [])) });
      } catch (e) {
        post({ kind: "reply", id: m.id, ok: false, error: String(e?.message ?? e) });
      }
    } else if (m.kind === "event") {
      if (m.name === "init") {
        initData = m.data;
        applyTheme(m.data.theme);
        resolveReady(m.data);
      } else if (m.name === "theme") applyTheme(m.data);
      emit(m.name, m.data);
    }
  });

  function applyTheme(theme) {
    if (!theme) return;
    const root = document.documentElement;
    for (const [k, v] of Object.entries(theme.vars ?? {})) root.style.setProperty(k, v);
    root.dataset.theme = theme.dark ? "dark" : "light";
  }

  // ---- bytes <-> base64 (the app moves binary data as base64) ----
  function toBase64(bytes) {
    if (bytes instanceof ArrayBuffer) bytes = new Uint8Array(bytes);
    let s = "";
    const chunk = 0x8000;
    for (let i = 0; i < bytes.length; i += chunk) s += String.fromCharCode.apply(null, bytes.subarray(i, i + chunk));
    return btoa(s);
  }
  function fromBase64(b64) {
    const s = atob(b64);
    const out = new Uint8Array(s.length);
    for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
    return out;
  }

  class HttpResponse {
    constructor(r) {
      this.status = r.status;
      this.ok = r.status >= 200 && r.status < 300;
      this.url = r.url;
      this.headers = r.headers;
      this._b64 = r.bodyBase64;
    }
    bytes() {
      return fromBase64(this._b64);
    }
    text() {
      return new TextDecoder().decode(this.bytes());
    }
    json() {
      const t = this.text();
      return t.trim() ? JSON.parse(t) : null;
    }
  }

  const xstudio = {
    apiVersion: 1,
    /** Resolves with {manifest, view, theme} once the app has connected. */
    ready,
    get manifest() {
      return initData?.manifest;
    },
    /** "section" or "settings" — which view the app is showing the page in. */
    get view() {
      return initData?.view ?? "section";
    },
    on(name, fn) {
      if (!listeners.has(name)) listeners.set(name, new Set());
      listeners.get(name).add(fn);
      return () => listeners.get(name).delete(fn);
    },

    http: {
      /**
       * fetch(url, {method, headers, body, json}) → HttpResponse {status, ok, headers, text(), json(), bytes()}.
       * Only https:// hosts listed in permissions.network. A header value may contain
       * {{secret:NAME}}: the app puts the stored secret there.
       */
      async fetch(url, opts = {}) {
        const params = { url, method: opts.method ?? "GET", headers: { ...(opts.headers ?? {}) } };
        if (opts.json !== undefined) {
          params.body = JSON.stringify(opts.json);
          params.headers["Content-Type"] ??= "application/json";
        } else if (typeof opts.body === "string") params.body = opts.body;
        else if (opts.body != null) params.bodyBase64 = toBase64(opts.body);
        return new HttpResponse(await call("http.fetch", params));
      },
    },

    /** Small JSON settings, kept per plugin (up to 1 MB). */
    storage: {
      get: (key) => call("storage.get", { key }),
      set: (key, value) => call("storage.set", { key, value }),
      delete: (key) => call("storage.delete", { key }),
      keys: () => call("storage.keys"),
    },

    /** Tokens: write-only. Use them as {{secret:NAME}} in http.fetch headers. */
    secrets: {
      set: (name, value) => call("secrets.set", { name, value }),
      has: (name) => call("secrets.has", { name }),
      delete: (name) => call("secrets.delete", { name }),
    },

    auth: {
      /**
       * Opens url in a sign-in window; when it redirects to a URL carrying `param`
       * (default access_token) the value is stored as secret `secret`. Rejects on cancel.
       */
      signIn: ({ url, secret, param }) => call("auth.signIn", { url, secret, param }),
    },

    device: {
      /** Sends a book to the reader (into /Books). data: Uint8Array. */
      sendBook: (name, data) => call("device.sendBook", { name, data: toBase64(data) }),
      /** Names of the book files on the reader (needs device: read). */
      books: () => call("device.books"),
    },

    epub: {
      /**
       * Cleans an EPUB from an online store: metadata into the OPF, a cover page, XHTML
       * repaired, embedded fonts dropped. Returns {data: Uint8Array, fileName}.
       * metadata: {title, authors[], annotation, genres[], publishers[], series[{title, positionLabel}],
       *            source, originalYear, rights, uuid}
       */
      async prepare(data, metadata = {}, cover = null) {
        const r = await call("epub.prepare", {
          data: toBase64(data),
          metadata,
          cover: cover ? toBase64(cover) : undefined,
        });
        return { data: fromBase64(r.data), fileName: r.fileName };
      },
    },

    files: {
      /** Asks the user where to save, then writes data there. Resolves false if cancelled. */
      saveAs: (name, data) => call("files.saveAs", { name, data: toBase64(data) }),
    },

    ui: {
      toast: (message, kind = "info") => call("ui.toast", { message, kind }),
      confirm: (message, opts = {}) => call("ui.confirm", { message, ...opts }),
    },

    /**
     * Retired in X Studio 0.2 (there is no "Get books" any more). Kept as a no-op so plugins
     * built for the earlier API still start. Methods are never removed from the SDK.
     */
    registerBookSource() {
      console.warn("xstudio.registerBookSource is retired and does nothing");
    },

    /**
     * Registers a book action declared in contributes.bookActions.
     * run({name, path, data: Uint8Array}) → {data: Uint8Array} to replace the book, or null.
     */
    registerBookAction(id, run) {
      handlers.set(`action.${id}`, run);
    },

    util: { toBase64, fromBase64 },
  };

  window.xstudio = xstudio;
  window.addEventListener("error", (e) => {
    const where = e.filename ? ` (${e.filename.split("/").pop()}:${e.lineno})` : "";
    post({ kind: "log", level: "error", message: `${e.error?.message ?? e.message}${where}` });
  });
  window.addEventListener("unhandledrejection", (e) => post({ kind: "log", level: "error", message: String(e.reason?.message ?? e.reason) }));
  post({ kind: "hello" });
})();
