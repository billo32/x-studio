<script>
  import { onMount } from "svelte";
  import { api, formatSize, joinPath } from "./api.js";
  import { FONT_SIZE_RANGES, fontsRoot, installedFonts } from "./models.js";

  // entries: everything on the card; files: the Fonts section's files.
  // transfers: the shell's queue rows; onQueue adds rows so the sidebar shows them right away.
  let { entries, files, query, transfers = [], onQueue, onDownload, onRemove, onInstalled, onError } = $props();

  let catalog = $state(null); // [{ name, description, languages, styles, files: [{ name, size, crc32, pointSize }] }]
  let loadError = $state("");
  let rangeId = $state(loadPref("fontRange", "medium"));
  let script = $state("all");
  let onlyInstalled = $state(false);
  let busy = $state({}); // family name → true while installing

  const range = $derived(FONT_SIZE_RANGES.find((r) => r.id === rangeId) ?? FONT_SIZE_RANGES[1]);
  const installed = $derived(installedFonts(entries));
  const root = $derived(fontsRoot(entries));

  const shown = $derived.by(() => {
    if (!catalog) return [];
    const q = query.trim().toLowerCase();
    return catalog.filter(
      (f) =>
        (!onlyInstalled || installed.has(f.name.toLowerCase())) &&
        (script === "all" || f.languages.toLowerCase().includes(script)) &&
        (!q || f.name.toLowerCase().includes(q) || f.description.toLowerCase().includes(q)),
    );
  });

  // Fonts on the card that the library doesn't know: own TTF families, older .xtf files.
  const extras = $derived.by(() => {
    if (!catalog || script !== "all") return [];
    const known = new Set(catalog.map((f) => f.name.toLowerCase()));
    const q = query.trim().toLowerCase();
    const out = [];
    for (const [key, fam] of installed) {
      if (known.has(key)) continue;
      const inside = files.filter((x) => x.path.startsWith(fam.path + "/"));
      out.push({ key: fam.path, name: fam.name, note: `${inside.length} ${inside.length === 1 ? "file" : "files"}`, size: inside.reduce((t, x) => t + x.size, 0), entry: { path: fam.path, name: fam.name, isDir: true, size: 0 } });
    }
    for (const x of files) {
      const parts = x.path.split("/").filter(Boolean);
      if (parts.length === 2) out.push({ key: x.path, name: x.name, note: `/${parts[0]}`, size: x.size, entry: x });
    }
    return out.filter((x) => !q || x.name.toLowerCase().includes(q));
  });

  function loadPref(k, d) {
    try {
      return localStorage.getItem(k) ?? d;
    } catch {
      return d;
    }
  }
  function setRange(id) {
    rangeId = id;
    try {
      localStorage.setItem("fontRange", id);
    } catch {}
  }

  async function load() {
    loadError = "";
    try {
      catalog = await api.fontCatalog();
    } catch (e) {
      loadError = String(e);
    }
  }
  onMount(load);

  /** Files of `f` in the chosen range (and what is already on the card). */
  function plan(f) {
    const want = f.files.filter((x) => range.sizes.includes(x.pointSize));
    const have = installed.get(f.name.toLowerCase());
    const missing = want.filter((x) => !have?.sizes.includes(x.pointSize));
    return { want, have, missing, bytes: want.reduce((s, x) => s + x.size, 0) };
  }

  const fileId = (f, name) => `font:${joinPath(joinPath(root, f.name), name)}`;

  /** Where an install stands, from the queue rows: { label, value } (value 0–1). */
  function progressOf(f, count) {
    const dl = transfers.find((t) => t.id === `fontdl:${f.name}`);
    if (dl && (dl.state === "running" || dl.state === "queued")) {
      return { label: "Downloading…", value: dl.total ? (dl.done / dl.total) * 0.15 : 0 };
    }
    const prefix = fileId(f, "");
    const rows = transfers.filter((t) => t.id.startsWith(prefix) && (t.state === "running" || t.state === "queued"));
    const running = rows.find((t) => t.state === "running");
    const finished = count - rows.length;
    if (!running) return { label: rows.length ? "Waiting for the reader…" : "Finishing…", value: 0.15 + (0.85 * finished) / count };
    const part = running.total ? running.done / running.total : 0;
    return { label: `Writing ${finished + 1} of ${count}`, value: 0.15 + (0.85 * (finished + part)) / count };
  }

  async function install(f) {
    const { missing } = plan(f);
    onQueue?.([
      { id: `fontdl:${f.name}`, name: `Downloading ${f.name}` },
      ...missing.map((x) => ({ id: fileId(f, x.name), name: x.name })),
    ]);
    busy[f.name] = missing.length;
    try {
      await api.installFont(f.name, missing.map((x) => x.pointSize), root);
      onInstalled();
    } catch (e) {
      onError(`${f.name}: ${e}`);
    } finally {
      busy[f.name] = false;
    }
  }

  function remove(f) {
    const have = installed.get(f.name.toLowerCase());
    if (have) onRemove({ path: have.path, name: have.name, isDir: true, size: 0 });
  }

  const styleCount = (f) => (f.styles?.length ? `${f.styles.length} ${f.styles.length === 1 ? "style" : "styles"}` : "");
  const sizesLabel = (sizes) => (sizes.length ? `${sizes.join(", ")} pt` : "TTF");
</script>

<section>
  <div class="lib-head">
    <div>
      <p class="hint">CrossInk's ready-made fonts, the same as <i>Manage Fonts</i> on the reader — installed straight from your Mac.</p>
    </div>
    <div class="filters">
      <div class="seg" role="group" aria-label="Show">
        <button class:on={!onlyInstalled} aria-pressed={!onlyInstalled} onclick={() => (onlyInstalled = false)}>All</button>
        <button class:on={onlyInstalled} aria-pressed={onlyInstalled} onclick={() => (onlyInstalled = true)}>On the reader</button>
      </div>
      <div class="seg" role="group" aria-label="Sizes">
        {#each FONT_SIZE_RANGES as r (r.id)}
          <button class:on={r.id === rangeId} aria-pressed={r.id === rangeId} onclick={() => setRange(r.id)}>{r.label}</button>
        {/each}
      </div>
      <div class="seg" role="group" aria-label="Script">
        {#each [["all", "All scripts"], ["cyrillic", "Cyrillic"], ["greek", "Greek"]] as [id, label] (id)}
          <button class:on={script === id} aria-pressed={script === id} onclick={() => (script = id)}>{label}</button>
        {/each}
      </div>
    </div>
  </div>

  {#if loadError}
    <div class="state">
      <p>{loadError}</p>
      <button class="link" onclick={load}>Try again</button>
    </div>
  {:else if !catalog}
    <p class="state"><span class="spinner"></span> Loading the font library…</p>
  {:else}
    <div class="grid">
      {#each shown as f (f.name)}
        {@const p = plan(f)}
        <article class="font" class:on={!!p.have}>
          <div class="top">
            <h3>{f.name}</h3>
            {#if p.have}<span class="badge">On the reader</span>{/if}
          </div>
          <p class="desc">{f.description}</p>
          <p class="meta">{[f.languages, styleCount(f)].filter(Boolean).join(" · ")}</p>
          <div class="foot">
            {#if busy[f.name]}
              {@const pr = progressOf(f, busy[f.name])}
              <div class="progress">
                <span class="row"><span>{pr.label}</span><span>{Math.round(pr.value * 100)}%</span></span>
                <progress max="1" value={pr.value}></progress>
              </div>
            {:else if p.want.length === 0}
              <span class="muted">No files in {range.label}</span>
            {:else if p.have && p.missing.length === 0}
              <span class="muted">{sizesLabel(p.have.sizes)}</span>
              <button class="link danger" onclick={() => remove(f)}>Remove</button>
            {:else}
              <button class={p.have ? "" : "primary"} onclick={() => install(f)} title={p.have ? `Add the missing ${range.label} files` : `Install ${range.label}`}>
                {p.have ? "Add sizes" : "Install"}
              </button>
              <span class="muted">{formatSize(p.missing.reduce((s, x) => s + x.size, 0))}</span>
              {#if p.have}<button class="link danger" onclick={() => remove(f)}>Remove</button>{/if}
            {/if}
          </div>
        </article>
      {:else}
        {#if extras.length === 0}<p class="state">{onlyInstalled ? "None of the library's fonts are on the reader yet." : "No fonts match."}</p>{/if}
      {/each}
      {#each extras as x (x.key)}
        <article class="font on">
          <div class="top">
            <h3>{x.name}</h3>
            <span class="badge">On the reader</span>
          </div>
          <p class="desc">Not from the library</p>
          <p class="meta">{x.note}</p>
          <div class="foot">
            <span class="muted">{formatSize(x.size)}</span>
            {#if !x.entry.isDir}<button class="link" onclick={() => onDownload(x.entry)}>Download</button>{/if}
            <button class="link danger" onclick={() => onRemove(x.entry)}>Remove</button>
          </div>
        </article>
      {/each}
    </div>
  {/if}
</section>

<style>
  section { display: flex; flex-direction: column; gap: 12px; }
  .lib-head { display: flex; flex-wrap: wrap; align-items: flex-end; justify-content: space-between; gap: 12px; }
  .hint { margin: 0; color: var(--ink-2); }
  .filters { display: flex; flex-wrap: wrap; gap: 8px; }
  .seg { display: flex; padding: 3px; background: var(--paper-2); border-radius: 9px; gap: 2px; }
  .seg button { height: 28px; padding: 0 10px; border: none; border-radius: 7px; background: transparent; color: var(--ink-2); font-size: 12px; white-space: nowrap; }
  .seg button.on { background: var(--sheet); color: var(--ink); box-shadow: 0 1px 2px rgb(0 0 0 / 0.12); }

  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(230px, 1fr)); gap: 12px; }
  .font { display: flex; flex-direction: column; gap: 4px; padding: 14px 16px 12px; background: var(--sheet); border: 1px solid var(--rule); border-radius: 10px; }
  .font.on { border-color: var(--rule-strong); }
  .top { display: flex; align-items: baseline; justify-content: space-between; gap: 8px; }
  h3 { margin: 0; font-size: 15px; font-weight: 600; }
  .badge { flex: none; font-size: 11px; color: var(--ok); }
  .desc { margin: 0; color: var(--ink-2); }
  .meta { margin: 0; color: var(--ink-3); font-size: 12px; }
  .foot { display: flex; align-items: center; gap: 10px; margin-top: auto; padding-top: 10px; min-height: 30px; }
  .foot button:not(.link) { flex: none; height: 30px; padding: 0 14px; white-space: nowrap; }
  .foot .link.danger { margin-left: auto; }
  .muted { white-space: nowrap; display: inline-flex; align-items: center; gap: 6px; color: var(--ink-3); font-size: 12px; }
  .progress { flex: 1; display: flex; flex-direction: column; gap: 6px; font-size: 12px; color: var(--ink-2); }
  .progress .row { display: flex; justify-content: space-between; font-variant-numeric: tabular-nums; }
  .progress progress { width: 100%; }
  .state { display: flex; align-items: center; gap: 8px; color: var(--ink-2); margin: 0; }
</style>
