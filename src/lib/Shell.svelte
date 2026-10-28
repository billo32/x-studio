<script>
  import { onMount, untrack } from "svelte";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { open as openDialog, save as saveDialog, ask } from "@tauri-apps/plugin-dialog";
  import { api, formatSize, count, baseName } from "./api.js";
  import { MODELS, modelById, detectModel, rememberModel, sectionOf, SECTIONS, destinationFor, installedFonts } from "./models.js";
  import BooksView from "./BooksView.svelte";
  import ImagesView from "./ImagesView.svelte";
  import FilesView from "./FilesView.svelte";
  import FontsView from "./FontsView.svelte";
  import BookEditor from "./BookEditor.svelte";
  import PluginsView from "./plugins/PluginsView.svelte";
  import { PluginHost, pluginUrl, currentTheme, bookActionsFor, toBase64, fromBase64, diagnose } from "./plugins/host.js";
  import { invoke } from "@tauri-apps/api/core";

  let { status, onDisconnect } = $props();

  let modelId = $state(detectModel(untrack(() => status)));
  const model = $derived(modelById(modelId));

  let entries = $state([]);
  let scanning = $state(false);
  let section = $state("books");
  let view = $state(loadPref("view", "grid"));
  let query = $state("");
  let error = $state("");
  let notice = $state(""); // non-error message (plugins' ui.toast)

  // ---- plugins ----
  let plugins = $state([]); // [{manifest, enabled, permissions}]
  const running = $derived(plugins.filter((p) => p.enabled));
  const pluginSections = $derived(running.filter((p) => p.manifest.contributes?.section || p.manifest.contributes?.settings));
  const host = new PluginHost({
    invoke,
    theme: () => currentTheme(),
    toast: (message, kind, m) => (kind === "error" ? (error = `${m.name}: ${message}`) : showNotice(`${m.name}: ${message}`)),
    confirm: (message, opts, m) => ask(message, { title: m.name, okLabel: opts.okLabel ?? "OK", cancelLabel: opts.cancelLabel ?? "Cancel" }),
    saveDialog: (name) => saveDialog({ defaultPath: name }),
    bookNames: () => bySection.books.map((b) => b.name),
    onSent: () => rescan(),
    log: (level, message, m) => {
      console[level === "error" ? "error" : "log"](`[${m.id}]`, message);
      if (level === "error") {
        pluginErrors[m.id] = [...(pluginErrors[m.id] ?? []), message].slice(-5);
        // A page that broke shouldn't just sit on "Loading…".
        error = `${m.name} hit an error: ${message}. Right-click → Inspect Element → Console has the details; reinstalling or updating the plugin may help.`;
      }
    },
    onReady: (id) => {
      ready[id] = true;
      delete stuck[id];
    },
  });

  // Plugin pages that haven't connected: shown with what the app could find out.
  let ready = $state({}); // id → true once its SDK said hello
  let loaded = $state({}); // id → the iframe fired "load"
  let stuck = $state({}); // id → [diagnostic lines]
  let pluginErrors = $state({}); // id → recent errors the page reported
  const STARTUP = 6000;
  function watchStart(p) {
    const id = p.manifest.id;
    ready[id] = false;
    loaded[id] = false;
    delete stuck[id];
    setTimeout(async () => {
      if (ready[id]) return;
      stuck[id] = await diagnose(p.manifest);
    }, STARTUP);
  }

  async function loadPlugins() {
    try {
      plugins = await api.plugins();
      host.setPlugins(running.map((p) => p.manifest));
    } catch (e) {
      error = `Plugins: ${e}`;
    }
  }

  let noticeTimer;
  function showNotice(text) {
    notice = text;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (notice = ""), 5000);
  }

  /** Keeps a plugin's iframe registered with the host while it exists. */
  function pluginFrame(node, p) {
    const id = p.manifest.id;
    host.attach(id, node);
    watchStart(p);
    const onLoad = () => (loaded[id] = true);
    node.addEventListener("load", onLoad);
    return {
      destroy: () => {
        node.removeEventListener("load", onLoad);
        host.detach(id);
      },
    };
  }

  function openPlugin(id, view = "section") {
    host.setView(id, view);
    openSection(`plugin:${id}`);
  }

  const editingActions = $derived(editing ? bookActionsFor(plugins, editing.name) : []);
  let actionBusy = $state("");
  async function runBookAction(plugin, action) {
    const entry = editing;
    actionBusy = `${plugin.id}/${action.id}`;
    try {
      const data = fromBase64(await api.readBookBytes(entry.path));
      const result = await host.invoke(plugin.id, `action.${action.id}`, { name: entry.name, path: entry.path, data });
      if (result?.data) {
        await api.writeBookBytes(entry.path, toBase64(result.data));
        const updated = { ...entry, size: result.data.length };
        entries = entries.map((e) => (e.path === entry.path ? updated : e));
        delete cards[entry.path];
        if (editing?.path === entry.path) editing = updated;
        api.bookInfo(updated.path, updated.size).then((c) => (cards[updated.path] = c)).catch(() => {});
        showNotice(`${action.title}: done`);
      }
    } catch (e) {
      error = `${action.title}: ${e.message ?? e}`;
    } finally {
      actionBusy = "";
    }
  }

  let cards = $state({}); // path → BookCard
  let thumbs = $state({}); // path → Preview
  let meta = $state({ done: 0, total: 0 }); // background book reading
  let transfers = $state([]); // {id, name, done, total, state, error}

  let editing = $state(null); // entry being edited
  let editor = $state(null);

  const files = $derived(entries.filter((e) => !e.isDir));
  const bySection = $derived.by(() => {
    const out = { books: [], images: [], fonts: [], dicts: [], other: [] };
    for (const f of files) out[sectionOf(f)].push(f);
    return out;
  });
  const usedBytes = $derived(files.reduce((s, f) => s + f.size, 0));
  const active = $derived(transfers.find((t) => t.state === "running") ?? transfers.find((t) => t.state === "queued"));
  const queued = $derived(transfers.filter((t) => t.state === "queued" || t.state === "running").length);

  function loadPref(k, d) {
    try {
      return localStorage.getItem(k) ?? d;
    } catch {
      return d;
    }
  }
  function setView(v) {
    view = v;
    try {
      localStorage.setItem("view", v);
    } catch {}
  }
  function setModel(id) {
    modelId = id;
    rememberModel(status.address, id);
  }

  // ---- loading ----
  let generation = 0;
  async function rescan() {
    scanning = true;
    const gen = ++generation;
    try {
      entries = await api.scan();
    } catch (e) {
      error = String(e);
    } finally {
      scanning = false;
    }
    if (gen === generation) readBooks(gen);
  }

  // Reads books in the background: title, author, cover, stats. The reader sends one file at
  // a time (about 400 KB/s over USB), so two requests are kept in flight: while one book is
  // parsed on the Mac, the next one is already coming over the cable.
  async function readBooks(gen) {
    const todo = bySection.books.filter((b) => !cards[b.path]);
    meta = { done: 0, total: todo.length };
    let next = 0;
    const worker = async () => {
      while (next < todo.length && gen === generation) {
        const b = todo[next++];
        try {
          cards[b.path] = await api.bookInfo(b.path, b.size);
        } catch (e) {
          console.warn("book_info", b.path, e);
        }
        if (gen === generation) meta = { done: meta.done + 1, total: meta.total };
      }
    };
    await Promise.all([worker(), worker()]);
    if (gen === generation && section === "images") readThumbs(gen);
  }

  let thumbsRunning = false;
  async function readThumbs(gen = generation) {
    if (thumbsRunning) return;
    thumbsRunning = true;
    try {
      for (const img of bySection.images) {
        if (gen !== generation) return;
        if (thumbs[img.path]) continue;
        try {
          thumbs[img.path] = await api.imageThumb(img.path, img.size);
        } catch (e) {
          console.warn("image_thumb", img.path, e);
        }
      }
    } finally {
      thumbsRunning = false;
    }
  }

  function openSection(id) {
    section = id;
    editing = null;
    query = "";
    if (id === "images") readThumbs();
  }

  // ---- transfers ----
  let nextId = 0;
  async function uploadPaths(paths) {
    if (!paths.length) return;
    const groups = new Map();
    for (const p of paths) {
      const dest = destinationFor(p, entries);
      groups.set(dest, [...(groups.get(dest) ?? []), p]);
    }
    for (const [dest, list] of groups) {
      const items = list.map((p) => [`up${nextId++}`, p]);
      transfers = [...transfers, ...items.map(([id, p]) => ({ id, name: baseName(p), done: 0, total: 0, state: "queued" }))];
      try {
        if (dest !== "/") await api.makeDir(dest).catch(() => {});
        // Pictures become proper sleep screens: screen-sized grayscale BMP.
        if (dest === "/sleep") await api.uploadSleep(items, dest, model.width, model.height);
        else await api.upload(items, dest, true);
      } catch (e) {
        error = String(e);
      }
    }
    rescan();
  }

  /** Adds rows to the transfer queue (events for them update them later). */
  function queueRows(rows) {
    const fresh = rows.filter((r) => !transfers.some((t) => t.id === r.id && (t.state === "queued" || t.state === "running")));
    transfers = [...transfers.filter((t) => !fresh.some((r) => r.id === t.id)), ...fresh.map((r) => ({ ...r, done: 0, total: 0, state: "queued" }))];
  }

  async function addFiles() {
    const filters = {
      books: [{ name: "Books", extensions: ["epub", "fb2", "txt", "xtc", "xtch"] }],
      images: [{ name: "Pictures", extensions: ["bmp", "png", "jpg", "jpeg", "webp"] }],
      fonts: [{ name: "Fonts", extensions: ["ttf", "otf", "cpfont"] }],
    }[section] ?? [];
    const picked = await openDialog({ multiple: true, filters: [...filters, { name: "All files", extensions: ["*"] }] });
    if (picked) uploadPaths(Array.isArray(picked) ? picked : [picked]);
  }

  async function download(entry) {
    const target = await saveDialog({ defaultPath: entry.name });
    if (!target) return;
    transfers = [...transfers, { id: `dl:${entry.path}`, name: entry.name, done: 0, total: entry.size, state: "queued" }];
    api.download(entry.path, target).catch((e) => (error = String(e)));
  }

  async function remove(entry) {
    const ok = await ask(`Delete “${entry.name}” from the reader${entry.isDir ? " with everything inside" : ""}?`, {
      title: "Delete",
      kind: "warning",
      okLabel: "Delete",
      cancelLabel: "Cancel",
    });
    if (!ok) return;
    try {
      await api.remove(entry.path);
      entries = entries.filter((e) => e.path !== entry.path && !e.path.startsWith(entry.path + "/"));
      if (editing?.path === entry.path) editing = null;
    } catch (e) {
      error = String(e);
    }
  }

  function onSaved(updated) {
    const old = editing;
    entries = [...entries.filter((e) => e.path !== old.path), updated];
    delete cards[old.path];
    editing = null;
    api.bookInfo(updated.path, updated.size).then((c) => (cards[updated.path] = c)).catch(() => {});
  }

  async function disconnect() {
    generation++;
    await api.disconnect().catch(() => {});
    onDisconnect();
  }

  let dragPaths = [];

  onMount(() => {
    rescan();
    host.start();
    loadPlugins();
    const dark = matchMedia("(prefers-color-scheme: dark)");
    const onScheme = () => host.broadcastTheme(currentTheme());
    dark.addEventListener("change", onScheme);
    const unlisten = [() => host.stop(), () => dark.removeEventListener("change", onScheme)];
    api.onTransfer((ev) => {
      let i = transfers.findIndex((t) => t.id === ev.id);
      if (i < 0) {
        transfers = [...transfers, { id: ev.id, name: ev.name, done: 0, total: 0, state: "queued" }];
        i = transfers.length - 1;
      }
      const t = transfers[i];
      if (ev.state === "running") transfers[i] = { ...t, done: ev.done, total: ev.total, state: "running" };
      else {
        transfers[i] = { ...t, done: t.total, state: ev.state, error: ev.error };
        if (ev.error) error = `${ev.name}: ${ev.error}`;
        setTimeout(() => (transfers = transfers.filter((x) => x.id !== ev.id || x.state === "running")), 4000);
      }
    }).then((u) => unlisten.push(u));

    getCurrentWebview()
      .onDragDropEvent((ev) => {
        const p = ev.payload;
        // "over" events carry only the position, so remember the paths from "enter".
        if (p.type === "enter") dragPaths = p.paths ?? [];
        if (p.type === "enter" || p.type === "over") {
          // In the editor a single image is meant for the cover: the cover lights up instead
          // of the upload overlay. Anything else still uploads.
          const coverDrag = editing && editor ? editor.dragAt(p.position, dragPaths) : null;
          dragging = !coverDrag;
        } else if (p.type === "leave") {
          dragging = false;
          editor?.dragEnd();
        } else if (p.type === "drop") {
          dragging = false;
          if (!(editing && editor && editor.dropAt(p.position, p.paths))) uploadPaths(p.paths);
        }
      })
      .then((u) => unlisten.push(u));
    return () => unlisten.forEach((u) => u());
  });

  let dragging = $state(false);

  // Each section and the editor start at the top, not at the previous scroll position.
  let contentEl = $state(null);
  $effect(() => {
    editing;
    section;
    if (contentEl) contentEl.scrollTop = 0;
  });

  const header = $derived.by(() => {
    const n = bySection[section]?.length ?? 0;
    switch (section) {
      case "books": {
        const pages = bySection.books.reduce((s, b) => s + (cards[b.path]?.pages ?? 0), 0);
        return { title: "Books", sub: `${count(n, "book")} on the reader` + (pages ? ` · ${count(pages, "page")}` : "") };
      }
      case "images": return { title: "Sleep & boot screens", sub: `${count(n, "image")} · drop pictures to add them as ${model.width}×${model.height} screens` };
      case "fonts": return { title: "Fonts", sub: `${count(installedFonts(entries).size, "family", "families")} on the reader` };
      case "dicts": return { title: "Dictionaries", sub: "StarDict dictionaries in /dictionaries" };
      default: return { title: "Other", sub: count(n, "file") };
    }
  });
</script>

<div class="shell">
  <nav class="sidebar" aria-label="Sections">
    <div class="brand">
      <svg width="20" height="28" viewBox="0 0 20 28" aria-hidden="true"><rect x="1" y="1" width="18" height="26" rx="3" fill="var(--sheet)" stroke="var(--ink)" stroke-width="2"></rect><path d="M5 7h10M5 11h10M5 15h10M5 19h6" stroke="var(--ink-3)" stroke-width="1.5"></path></svg>
      X Studio
    </div>

    <div class="device">
      <div class="device-row">
        <span class="dot"></span>
        <label class="model">
          <span class="sr">Model</span>
          <select value={modelId} onchange={(e) => setModel(e.currentTarget.value)}>
            {#each MODELS as m (m.id)}<option value={m.id}>{m.name}</option>{/each}
          </select>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M7 10l5 5 5-5"></path></svg>
        </label>
      </div>
      <span class="sub">{status.transport === "usb" ? "USB" : "Wi‑Fi"} · {model.width}×{model.height}{status.firmware ? ` · ${status.firmware}` : ""}</span>
      <span class="sub">{scanning ? "Reading the SD card…" : `${count(files.length, "file")} · ${formatSize(usedBytes)}`}</span>
      <div class="device-actions">
        <button class="link" onclick={rescan} disabled={scanning}>Refresh</button>
        <!-- Over USB the app reconnects on its own, so there is nothing to disconnect. -->
        {#if status.transport !== "usb"}<button class="link" onclick={disconnect}>Disconnect</button>{/if}
      </div>
    </div>

    <div class="sections">
      {#each SECTIONS as s (s.id)}
        <button class="section" class:on={section === s.id && !editing} onclick={() => openSection(s.id)}>
          <span class="icon" aria-hidden="true">
            {#if s.id === "books"}<svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M4 4h5a3 3 0 013 3v13a2 2 0 00-2-2H4zM20 4h-5a3 3 0 00-3 3v13a2 2 0 012-2h6z"></path></svg>
            {:else if s.id === "images"}<svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><rect x="5" y="2.5" width="14" height="19" rx="2"></rect><path d="M5 16l4-4 3 3 2-2 5 5"></path></svg>
            {:else if s.id === "fonts"}<svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M4 19L9 5l5 14M6 14h6M15 19v-6a3 3 0 016 0v6M15 16h6"></path></svg>
            {:else if s.id === "dicts"}<svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M5 3h12a2 2 0 012 2v16H7a2 2 0 01-2-2z"></path><path d="M5 19a2 2 0 012-2h12M9 8h6"></path></svg>
            {:else}<svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2z"></path></svg>{/if}
          </span>
          <span class="label">{s.label}</span>
          <span class="count">{bySection[s.id].length}</span>
        </button>
      {/each}
      <span class="divider" aria-hidden="true"></span>
      {#each pluginSections.filter((p) => p.manifest.contributes?.section) as p (p.manifest.id)}
        <button class="section" class:on={section === `plugin:${p.manifest.id}` && !editing} onclick={() => openPlugin(p.manifest.id)}>
          <span class="icon" aria-hidden="true">
            {#if p.manifest.icon}<img class="picon" src={pluginUrl(p.manifest.id, p.manifest.icon)} alt="" />
            {:else}<svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M9 4h6v3a2 2 0 104 0V4h1v16H4V4h1v3a2 2 0 104 0z"></path></svg>{/if}
          </span>
          <span class="label">{p.manifest.contributes.section.title}</span>
        </button>
      {/each}
      <button class="section" class:on={section === "plugins" && !editing} onclick={() => openSection("plugins")}>
        <span class="icon" aria-hidden="true"><svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M10 4a2 2 0 114 0v2h4v4h-2a2 2 0 100 4h2v4h-4v-2a2 2 0 10-4 0v2H6v-4h2a2 2 0 100-4H6V6h4z"></path></svg></span>
        <span class="label">Plugins</span>
        <span class="count">{running.length || ""}</span>
      </button>
    </div>

    {#if active}
      <div class="task">
        <span class="task-row"><span>{queued > 1 ? `Transfer · ${queued - 1} more` : "Transfer"}</span><span>{active.total ? Math.floor((active.done / active.total) * 100) + "%" : "…"}</span></span>
        <progress max={active.total || 1} value={active.done}></progress>
        <span class="task-name">{active.name}</span>
      </div>
    {:else if meta.done < meta.total}
      <div class="task">
        <span class="task-row"><span>Reading books</span><span>{meta.done} of {meta.total}</span></span>
        <progress max={meta.total} value={meta.done}></progress>
        <span class="task-name">Titles, authors and covers — once, then from cache</span>
      </div>
    {/if}
  </nav>

  <main class="content" bind:this={contentEl}>
    {#if error}
      <div class="banner" role="alert"><span>{error}</span><button class="link" onclick={() => (error = "")}>Dismiss</button></div>
    {/if}
    {#if notice}
      <div class="banner info" role="status"><span>{notice}</span><button class="link" onclick={() => (notice = "")}>Dismiss</button></div>
    {/if}

    <!-- Every running plugin keeps its page loaded (it may serve book actions);
         only the open one is shown. -->
    <div class="frames" class:shown={!editing && section.startsWith("plugin:")}>
      {#each running as p (`${p.manifest.id}@${p.manifest.version}`)}
        <iframe
          use:pluginFrame={p}
          class:on={section === `plugin:${p.manifest.id}`}
          title={p.manifest.name}
          src={pluginUrl(p.manifest.id, p.manifest.entry)}
          sandbox="allow-scripts"
          referrerpolicy="no-referrer"
          allow=""
        ></iframe>
        {#if section === `plugin:${p.manifest.id}` && stuck[p.manifest.id]}
          <div class="stuck">
            <p class="big">{p.manifest.name} didn't start</p>
            <p>Its page didn't connect to X Studio. What the app could check:</p>
            <ul>
              <li>Frame load event: {loaded[p.manifest.id] ? "yes" : "no"}</li>
              {#each stuck[p.manifest.id] as line (line)}<li>{line}</li>{/each}
              {#each pluginErrors[p.manifest.id] ?? [] as e (e)}<li>Page error: {e}</li>{/each}
            </ul>
            <p class="hint">Right-click here → Inspect Element → Console shows the full reason (look for “Refused”, “blocked” or a 404).</p>
            <button onclick={() => { const f = document.querySelector(`iframe[title="${p.manifest.name}"]`); if (f) { watchStart(p); f.src = f.src; } }}>Reload plugin page</button>
          </div>
        {/if}
      {/each}
    </div>

    {#if editing}
      {#if editingActions.length}
        <div class="plugin-actions">
          <span class="pa-label">Plugins</span>
          {#each editingActions as { plugin, action } (`${plugin.id}/${action.id}`)}
            <button onclick={() => runBookAction(plugin, action)} disabled={!!actionBusy} title={plugin.name}>
              {actionBusy === `${plugin.id}/${action.id}` ? "Working…" : action.title}
            </button>
          {/each}
        </div>
      {/if}
      {#key editing.path}
        <BookEditor bind:this={editor} entry={editing} card={cards[editing.path]} {modelId} deviceName={model.name} onClose={() => (editing = null)} {onSaved} onRemove={remove} />
      {/key}
    {:else if section === "plugins"}
      <PluginsView {plugins} onChanged={loadPlugins} onOpenSettings={(id) => openPlugin(id, "settings")} onError={(e) => (error = e)} />
    {:else if section.startsWith("plugin:")}
      <!-- the plugin's frame is shown above -->
    {:else}
      <div class="page">
        <header class="head">
          <div class="titles">
            <h1>{header.title}</h1>
            <p>{header.sub}</p>
          </div>
          <div class="tools">
            <label class="search">
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><circle cx="11" cy="11" r="7"></circle><path d="M20 20l-3.5-3.5"></path></svg>
              <span class="sr">Search</span>
              <input bind:value={query} placeholder={section === "books" ? "Title or author" : section === "fonts" ? "Font name" : "File name"} />
            </label>
            {#if section === "books"}
              <div class="seg" role="group" aria-label="View">
                <button aria-label="Grid" aria-pressed={view === "grid"} class:on={view === "grid"} onclick={() => setView("grid")}>
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><rect x="4" y="4" width="6.5" height="6.5" rx="1"></rect><rect x="13.5" y="4" width="6.5" height="6.5" rx="1"></rect><rect x="4" y="13.5" width="6.5" height="6.5" rx="1"></rect><rect x="13.5" y="13.5" width="6.5" height="6.5" rx="1"></rect></svg>
                </button>
                <button aria-label="List" aria-pressed={view === "list"} class:on={view === "list"} onclick={() => setView("list")}>
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M9 6h11M9 12h11M9 18h11M4 6h.01M4 12h.01M4 18h.01"></path></svg>
                </button>
              </div>
            {/if}
            <button class="primary" onclick={addFiles}>Add…</button>
          </div>
        </header>

        {#if scanning && entries.length === 0}
          <p class="loading"><span class="spinner"></span> Reading the SD card…</p>
        {:else if section === "books"}
          <BooksView books={bySection.books} {cards} {view} {query} onOpen={(b) => (editing = b)} />
        {:else if section === "images"}
          <ImagesView images={bySection.images} {thumbs} {model} {query} onDownload={download} onRemove={remove} />
        {:else if section === "fonts"}
          <FontsView {entries} files={bySection.fonts} {query} {transfers} onQueue={queueRows} onDownload={download} onRemove={remove} onInstalled={rescan} onError={(e) => (error = e)} />
        {:else if section === "dicts"}
          <FilesView files={bySection.dicts} grouped {query} emptyTitle="No dictionaries" emptyText="Put a StarDict folder (.dict, .idx, .ifo) into /dictionaries." onDownload={download} onRemove={remove} />
        {:else}
          <FilesView files={bySection.other} {query} emptyTitle="Nothing else here" emptyText="Files that are not books, images, fonts or dictionaries show up here." onDownload={download} onRemove={remove} />
        {/if}

        <p class="footnote">Drop files onto the window — books go to /Books, pictures become sleep screens in /sleep, fonts go to /fonts.</p>
      </div>
    {/if}
  </main>

  {#if dragging}
    <div class="dropzone">
      <div>Release to upload to the reader</div>
    </div>
  {/if}
</div>

<style>
  .shell { display: grid; grid-template-columns: 248px minmax(0, 1fr); height: 100%; }

  .sidebar { display: flex; flex-direction: column; gap: 16px; padding: 20px 14px 16px; background: var(--paper-2); border-right: 1px solid var(--rule); overflow-y: auto; }
  .brand { display: flex; align-items: center; gap: 10px; padding: 0 6px; font-family: var(--serif); font-weight: 600; font-size: 17px; }

  .device { display: flex; flex-direction: column; gap: 4px; padding: 12px; background: var(--sheet); border: 1px solid var(--rule); border-radius: 10px; }
  .device-row { display: flex; align-items: center; gap: 8px; }
  .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--ok); flex: none; }
  .model { position: relative; display: flex; align-items: center; flex: 1; }
  .model select { appearance: none; -webkit-appearance: none; width: 100%; border: none; background: transparent; font: inherit; font-size: 14px; font-weight: 600; color: var(--ink); padding: 2px 20px 2px 0; cursor: pointer; }
  .model svg { position: absolute; right: 0; pointer-events: none; color: var(--ink-2); }
  .sub { color: var(--ink-2); font-size: 12px; }
  .device-actions { display: flex; gap: 8px; margin: 4px 0 0 -4px; }

  .sections { display: flex; flex-direction: column; gap: 2px; }
  .section { display: flex; align-items: center; gap: 10px; min-height: 36px; padding: 0 10px; border: none; border-radius: 8px; background: transparent; text-align: left; color: var(--ink-2); }
  .section:hover { background: color-mix(in srgb, var(--sheet) 60%, transparent); }
  .section.on { background: var(--sheet); color: var(--ink); font-weight: 600; box-shadow: 0 1px 2px rgb(0 0 0 / 0.08); }
  .icon { display: flex; width: 18px; justify-content: center; }
  .label { flex: 1; }
  .divider { height: 1px; margin: 8px 10px; background: var(--rule); }
  .picon { width: 17px; height: 17px; object-fit: contain; }

  .frames { display: none; position: absolute; inset: 0; }
  .frames.shown { display: block; }
  .frames iframe { display: none; width: 100%; height: 100%; border: 0; background: var(--paper); }
  .frames iframe.on { display: block; }
  .stuck { position: absolute; inset: 0; z-index: 1; padding: 40px 32px; background: var(--paper); display: flex; flex-direction: column; gap: 10px; align-items: flex-start; overflow: auto; }
  .stuck p { margin: 0; color: var(--ink-2); }
  .stuck .big { font-family: var(--serif); font-size: 20px; color: var(--ink); }
  .stuck ul { margin: 0; padding-left: 18px; color: var(--ink-2); font-family: var(--mono); font-size: 12px; display: flex; flex-direction: column; gap: 3px; user-select: text; -webkit-user-select: text; }
  .stuck .hint { font-size: 12px; color: var(--ink-3); }
  .banner { position: relative; z-index: 2; }
  .banner.info { border-color: var(--rule-strong); color: var(--ink-2); background: var(--sheet); }
  .plugin-actions { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; padding: 12px 32px 0; }
  .pa-label { font-size: 11px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--ink-3); }
  .count { color: var(--ink-3); font-variant-numeric: tabular-nums; font-weight: 400; }

  .task { margin-top: auto; display: flex; flex-direction: column; gap: 7px; padding: 12px; border: 1px dashed var(--ink-3); border-radius: 10px; }
  .task-row { display: flex; justify-content: space-between; font-size: 12px; }
  .task-row span:last-child { color: var(--ink-2); font-variant-numeric: tabular-nums; }
  .task-name { color: var(--ink-3); font-size: 11px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }

  .content { overflow-y: auto; min-width: 0; position: relative; }
  .page { padding: 26px 32px 28px; display: flex; flex-direction: column; gap: 20px; min-height: 100%; box-sizing: border-box; }
  .head { display: flex; flex-wrap: wrap; align-items: flex-end; gap: 16px; }
  .titles { flex: 1 1 260px; }
  h1 { margin: 0; font-family: var(--serif); font-weight: 600; font-size: 30px; line-height: 1.1; }
  .titles p { margin: 6px 0 0; color: var(--ink-2); }
  .tools { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; }
  .search { display: flex; align-items: center; gap: 8px; height: 36px; padding: 0 12px; background: var(--sheet); border: 1px solid var(--rule); border-radius: 8px; color: var(--ink-3); }
  .search input { border: none; background: none; padding: 0; width: 170px; outline: none; min-height: 0; }
  .seg { display: flex; padding: 3px; background: var(--paper-2); border-radius: 9px; gap: 2px; }
  .seg button { display: grid; place-items: center; width: 36px; height: 30px; padding: 0; border: none; border-radius: 7px; background: transparent; color: var(--ink-3); }
  .seg button.on { background: var(--sheet); color: var(--ink); box-shadow: 0 1px 2px rgb(0 0 0 / 0.12); }
  .tools .primary { height: 36px; padding: 0 16px; font-weight: 500; }
  .loading { display: flex; align-items: center; gap: 8px; color: var(--ink-2); }
  .footnote { margin: auto 0 0; padding-top: 12px; color: var(--ink-3); font-size: 12px; text-align: center; }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); }

  .dropzone { position: fixed; inset: 10px; border: 2px dashed var(--ink); border-radius: 14px; background: color-mix(in srgb, var(--paper) 85%, transparent); display: grid; place-items: center; font-size: 16px; pointer-events: none; z-index: 10; }
</style>
