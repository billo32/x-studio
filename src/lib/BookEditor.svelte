<script>
  import { onMount, untrack } from "svelte";
  import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
  import Cover from "./Cover.svelte";
  import CoverCanvas from "./CoverCanvas.svelte";
  import { api, formatSize, formatNumber, formatDuration, plural, parentPath } from "./api.js";
  import { MODELS, modelById, bookFormat } from "./models.js";
  import { FULL, cropSize, describeLayout, fillFor, gaps, hasBars, placeLayout, rgb, suggestLayout } from "./geometry.js";

  let { entry, card, modelId, deviceName, onClose, onSaved, onRemove } = $props();

  // ---- form ----
  // The editor may open before the book was read; the form fills in when its card arrives.
  const snapshot = (c) => ({
    title: c?.title ?? "",
    author: c?.authors?.join(", ") ?? "",
    language: c?.language ?? "",
    series: c?.series ?? "",
    folder: parentPath(untrack(() => entry.path)),
  });
  const first = snapshot(untrack(() => card));
  let initial = $state(first);
  let filled = untrack(() => !!card);
  let title = $state(first.title);
  let author = $state(first.author);
  let language = $state(first.language);
  let series = $state(first.series);
  let folder = $state(first.folder);
  $effect(() => {
    if (!filled && card) {
      filled = true;
      initial = snapshot(card);
      ({ title, author, language, series, folder } = initial);
    }
  });

  // ---- cover ----
  let current = $state(null); // Preview of the book's cover (≤480×800 data URL + original size)
  let replacement = $state(null); // { path, preview }
  // Layout: crop off baked-in bars, fit, zoom, shift, and how the rest of the screen is filled.
  let trim = $state(false);
  let fit = $state("cover"); // cover | contain | stretch
  let zoom = $state(1);
  let dx = $state(0);
  let dy = $state(0);
  let fill = $state("extend"); // extend | blur | white | black
  let coverTouched = $state(false); // re-bake the cover on save
  let previewModel = $state(untrack(() => modelId));
  let eink = $state(true);
  // Drag and drop onto the cover: "near" while an image is dragged over the window,
  // "over" while it is above the cover itself.
  let drag = $state(null); // null | "near" | "over"
  let coverEl = $state(null);

  let saving = $state(false);
  let error = $state("");

  const editable = $derived(card?.editable ?? false);
  const model = $derived(modelById(previewModel));
  const image = $derived(replacement?.preview ?? current);
  const dirty = $derived(
    title !== initial.title || author !== initial.author || language !== initial.language ||
      series !== initial.series || folder.trim() !== initial.folder || coverTouched,
  );

  // The preview shrinks with its column (the two columns always stay side by side).
  let previewWidth = $state(360);
  const H = $derived(Math.round(Math.max(250, Math.min(380, ((previewWidth - 32) * model.height) / model.width))));
  const W = $derived((H * model.width) / model.height);
  const analysis = $derived(image?.analysis ?? null);
  const crop = $derived(trim && analysis ? analysis.trim : FULL);
  const geom = $derived.by(() => {
    if (!image) return null;
    const [cw, ch] = cropSize(image, crop);
    return placeLayout(cw, ch, W, H, { fit, zoom, dx, dy });
  });
  const uncovered = $derived(geom ? gaps(geom, W, H) : { any: false });
  const fitNote = $derived(geom ? describeLayout(geom, W, H, model.height / H, fit, fill) : "");
  const barsPx = $derived(analysis && hasBars(analysis) ? Math.round((1 - analysis.trim.h) * image.height + (1 - analysis.trim.w) * image.width) : 0);

  // A new image starts from a suggested layout; it is applied only once the user touches it
  // (or presses Apply), so opening a book never changes it by itself.
  let suggestedFor = null;
  $effect(() => {
    const img = image;
    if (!img || img === suggestedFor) return;
    suggestedFor = img;
    const s = suggestLayout(img, model.width, model.height);
    ({ trim, fit, zoom, dx, dy, fill } = s);
  });

  function touch() {
    coverTouched = true;
  }
  function setFit(f) {
    fit = f;
    zoom = 1;
    dx = dy = 0;
    touch();
  }
  function setZoom(z) {
    zoom = Math.min(3, Math.max(0.3, z));
    touch();
  }
  function moveBy(ddx, ddy) {
    dx += ddx;
    dy += ddy;
    touch();
  }
  function resetPosition() {
    zoom = 1;
    dx = dy = 0;
    touch();
  }

  /** Called by the shell when an image is dropped on the window. */
  export async function setImage(path) {
    if (!editable) return;
    try {
      replacement = { path, preview: await api.loadLocalImage(path) };
      coverTouched = true;
      error = "";
    } catch (e) {
      error = String(e);
    }
  }

  const IMAGE_EXT = /\.(jpe?g|png|webp|gif|bmp)$/i;

  function overCover(pos) {
    if (!coverEl || !pos) return false;
    const r = coverEl.getBoundingClientRect();
    const k = window.devicePixelRatio || 1; // the drag position comes in physical pixels
    const x = pos.x / k, y = pos.y / k;
    return x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
  }

  /** Window drag moved; returns null (not a cover image), "near" or "over". */
  export function dragAt(pos, paths) {
    const isImage = editable && paths?.length === 1 && IMAGE_EXT.test(paths[0]);
    drag = isImage ? (overCover(pos) ? "over" : "near") : null;
    return drag;
  }

  export function dragEnd() {
    drag = null;
  }

  /** Window drop; returns true if the editor handled it. A single image is always meant
   *  for the cover: dropped on it, it replaces the cover; dropped beside it, nothing happens
   *  (rather than an unexpected upload to /sleep). */
  export function dropAt(pos, paths) {
    const isImage = editable && paths?.length === 1 && IMAGE_EXT.test(paths[0]);
    drag = null;
    if (isImage && overCover(pos)) setImage(paths[0]);
    return isImage;
  }

  function resetCover() {
    replacement = null;
    coverTouched = false;
    suggestedFor = null;
  }

  async function chooseImage() {
    const p = await openDialog({ filters: [{ name: "Images", extensions: ["jpg", "jpeg", "png", "webp", "gif", "bmp"] }] });
    if (p) setImage(p);
  }

  function input() {
    const changed = (a, b) => (a.trim() !== b ? a.trim() : null);
    return {
      title: changed(title, initial.title),
      author: changed(author, initial.author),
      language: changed(language, initial.language),
      series: changed(series, initial.series),
      folder: folder.trim() !== initial.folder ? folder.trim() : null,
      cover: coverTouched
        ? {
            source: replacement?.path ?? null,
            width: model.width,
            height: model.height,
            layout: { crop: trim && analysis ? analysis.trim : null, fit, zoom, dx, dy, fill: fillFor(fill, analysis) },
          }
        : null,
    };
  }

  async function save() {
    saving = true;
    error = "";
    try {
      const updated = await api.saveBook(entry.path, entry.size, input());
      onSaved(updated);
    } catch (e) {
      error = String(e);
    } finally {
      saving = false;
    }
  }

  async function exportCopy() {
    const target = await saveDialog({ defaultPath: entry.name, filters: [{ name: "EPUB", extensions: ["epub"] }] });
    if (!target) return;
    saving = true;
    try {
      await api.exportBook(entry.path, entry.size, input(), target);
    } catch (e) {
      error = String(e);
    } finally {
      saving = false;
    }
  }

  onMount(async () => {
    try {
      current = await api.bookCover(entry.path, entry.size);
    } catch (e) {
      error = String(e);
    }
  });
</script>

<div class="editor">
  <header>
    <button class="back" onclick={onClose}>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M15 6l-6 6 6 6"></path></svg>
      Books
    </button>
    <span class="sep">/</span>
    <span class="crumb">{title || entry.name}</span>
    <span class="grow"></span>
    {#if editable}
      <button class="ghost-btn" onclick={exportCopy} disabled={saving}>Save EPUB to Mac…</button>
      <button class="primary" onclick={save} disabled={saving || !dirty}>
        {#if saving}<span class="spinner light"></span>{/if}
        {dirty ? `Update on ${deviceName}` : "No changes"}
      </button>
    {/if}
  </header>

  {#if error}<div class="banner" role="alert"><span>{error}</span><button class="link" onclick={() => (error = "")}>Dismiss</button></div>{/if}

  <div class="body">
    <!-- Preview -->
    <section class="preview" aria-label="Cover on the reader" bind:clientWidth={previewWidth}>
      <div class="seg" role="group" aria-label="Preview model">
        {#each [MODELS[0], MODELS[3]] as m (m.id)}
          <button class:on={model.width === m.width && model.height === m.height} onclick={() => (previewModel = m.id)}>
            {m.id === "x3" ? "X3" : "X4"} · {m.width}×{m.height}
          </button>
        {/each}
      </div>

      <div class="bezel" class:near={drag === "near"} class:over={drag === "over"}>
        <div class="screen-wrap" bind:this={coverEl} style:width="{W}px" style:height="{H}px">
          {#if image}
            <CoverCanvas {image} {crop} {fit} {zoom} {dx} {dy} {fill} {W} {H} {eink} interactive={editable} onMove={moveBy} onZoom={setZoom} />
          {:else}
            <button class="screen typo" class:eink onclick={chooseImage} disabled={!editable} aria-label="Choose a cover image">
              <Cover {title} {author} />
            </button>
          {/if}
          {#if editable && (drag || !image)}
            <span class="drop-hint" class:show={!!drag} aria-hidden="true">
              <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M12 16V4M7 9l5-5 5 5M4 16v3a1 1 0 001 1h14a1 1 0 001-1v-3"></path></svg>
              <span>{drag === "over" ? "Release to use as the cover" : drag === "near" ? "Drop it here" : "Click or drop an image"}</span>
            </span>
          {/if}
        </div>
      </div>

      {#if image}
        {#if editable}<p class="hint center">Drag to move · scroll or pinch to zoom · drop an image to replace</p>{/if}
        <div class="pills" role="radiogroup" aria-label="Cover fit">
          {#each [["cover", "Fill screen"], ["contain", "Fit whole"], ["stretch", "Stretch"]] as [id, label] (id)}
            <button role="radio" aria-checked={fit === id} class:on={fit === id} onclick={() => setFit(id)} disabled={!editable}>{label}</button>
          {/each}
        </div>

        {#if editable}
          <div class="zoom">
            <span>Zoom</span>
            <input type="range" min="0.3" max="3" step="0.01" value={zoom} oninput={(e) => setZoom(Number(e.currentTarget.value))} aria-label="Zoom" />
            <span class="num">{Math.round(zoom * 100)}%</span>
            <button class="link" onclick={resetPosition} disabled={zoom === 1 && dx === 0 && dy === 0}>Reset</button>
          </div>
        {/if}

        {#if uncovered.any || fit === "contain"}
          <div class="seg small" role="radiogroup" aria-label="Edges">
            <button class:on={fill === "extend"} onclick={() => ((fill = "extend"), touch())} disabled={!editable} title="Continue the cover's own background to the screen edges">
              {#if analysis}<span class="swatch" style:background="linear-gradient({rgb(analysis.top.color)} 50%, {rgb(analysis.bottom.color)} 50%)"></span>{/if}
              Extend
            </button>
            <button class:on={fill === "blur"} onclick={() => ((fill = "blur"), touch())} disabled={!editable}>Blur</button>
            <button class:on={fill === "white"} onclick={() => ((fill = "white"), touch())} disabled={!editable}>White</button>
            <button class:on={fill === "black"} onclick={() => ((fill = "black"), touch())} disabled={!editable}>Black</button>
          </div>
        {/if}

        {#if barsPx}
          <label class="check small"><input type="checkbox" bind:checked={trim} onchange={touch} disabled={!editable} /> Remove the old bars ({barsPx} px)</label>
        {/if}

        <p class="note">{fitNote}</p>
        {#if editable && !coverTouched}
          <p class="hint center">Suggested layout, not applied yet · <button class="link" onclick={touch}>Apply</button></p>
        {/if}
      {/if}

      <label class="check"><input type="checkbox" bind:checked={eink} /> Preview as e-ink</label>

      {#if replacement}
        <p class="replaced">Cover replaced with {replacement.path.split("/").pop()} · <button class="link" onclick={resetCover}>Restore original</button></p>
      {:else if editable && image}
        <button class="link small" onclick={chooseImage}>Replace image…</button>
      {/if}
    </section>

    <!-- Details -->
    <section class="details" aria-label="Book details">
      {#if !editable}
        <p class="readonly">{bookFormat(entry.name)} is view-only for now — editing arrives with the EPUB converter.</p>
      {/if}
      <label class="field">
        <span>Title</span>
        <input class="title-input" bind:value={title} disabled={!editable} />
      </label>
      <div class="two">
        <label class="field"><span>Author</span><input bind:value={author} disabled={!editable} /></label>
        <label class="field"><span>Language</span><input bind:value={language} placeholder="en" disabled={!editable} /></label>
        <label class="field"><span>Series</span><input bind:value={series} placeholder="No series" disabled={!editable} /></label>
        <label class="field"><span>Folder on reader</span><input class="mono" bind:value={folder} disabled={!editable} /></label>
      </div>

      {#if card}
        <div class="stats">
          <div><b>{formatNumber(card.pages)}</b><span>{plural(card.pages, "page", "pages")}</span></div>
          <div><b>{card.chapters}</b><span>{plural(card.chapters, "chapter", "chapters")}</span></div>
          <div><b>{card.images}</b><span>{plural(card.images, "image", "images")}</span></div>
          <div><b>{formatDuration(card.readingMinutes)}</b><span>to read</span></div>
        </div>
        <p class="hint">Stable pages of 1,500 characters — they don’t change with the font. File: {formatSize(entry.size)}, {formatNumber(card.chars)} characters.</p>
      {:else}
        <p class="hint"><span class="spinner"></span> Reading the book from the reader…</p>
      {/if}

      <div class="actions">
        <span class="where mono">{entry.path}</span>
        <span class="grow"></span>
        <button class="danger-btn" onclick={() => onRemove(entry)} disabled={saving}>Delete from reader</button>
      </div>
    </section>
  </div>
</div>

<style>
  .editor { display: flex; flex-direction: column; min-height: 100%; }
  header { display: flex; align-items: center; gap: 10px; padding: 12px clamp(16px, 2.5vw, 28px); border-bottom: 1px solid var(--rule); background: var(--paper-2); }
  .back { display: flex; align-items: center; gap: 6px; border: none; background: none; padding: 6px 8px 6px 2px; min-height: 36px; color: var(--ink-2); }
  .back:hover { color: var(--ink); }
  .sep { color: var(--ink-3); }
  .crumb { font-weight: 500; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; flex: 0 1 auto; }
  header > button:not(.back) { flex: none; white-space: nowrap; min-height: 36px; padding: 0 16px; border-radius: 8px; display: flex; align-items: center; gap: 8px; }
  .back, .sep { flex: none; }
  .ghost-btn { background: transparent; }
  .where { color: var(--ink-3); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }

  /* Two columns that shrink together and never stack. */
  .body { flex: 1; display: flex; gap: clamp(16px, 3vw, 36px); padding: 26px clamp(16px, 2.5vw, 28px) 28px; }
  .preview { flex: 1 1 0; max-width: 380px; min-width: 220px; display: flex; flex-direction: column; align-items: center; gap: 14px; }
  .details { flex: 1.4 1 0; min-width: 240px; display: flex; flex-direction: column; gap: 16px; container-type: inline-size; }
  .pills button { white-space: nowrap; }

  .seg { display: flex; padding: 3px; background: var(--paper-2); border-radius: 9px; gap: 2px; }
  .seg button { border: none; background: transparent; min-height: 30px; padding: 0 12px; border-radius: 7px; font-size: 12px; color: var(--ink-2); }
  .seg button.on { background: var(--sheet); color: var(--ink); box-shadow: 0 1px 2px rgb(0 0 0 / 0.12); }

  .bezel { padding: 20px 16px 32px; background: #2b2a27; border-radius: 22px; box-shadow: 0 24px 48px -28px rgb(0 0 0 / 0.6); transition: box-shadow 0.2s, transform 0.2s; }
  .bezel.near { box-shadow: 0 0 0 2px var(--ink-3), 0 24px 48px -28px rgb(0 0 0 / 0.6); }
  .bezel.over { box-shadow: 0 0 0 3px var(--ink), 0 24px 48px -28px rgb(0 0 0 / 0.6); transform: scale(1.015); }
  .screen-wrap { position: relative; display: block; border-radius: 3px; }
  .screen.typo { padding: 0; border: none; cursor: pointer; width: 100%; }
  .screen.typo:disabled { cursor: default; opacity: 1; }
  .drop-hint {
    position: absolute; inset: 0; z-index: 2; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 10px;
    border-radius: 3px; background: rgb(20 19 17 / 0.62); color: #f2f0ea; font-size: 13px; font-weight: 500; text-align: center; padding: 20px;
    opacity: 0; transition: opacity 0.15s;
  }
  .drop-hint { pointer-events: none; }
  .drop-hint.show, .screen-wrap:hover .drop-hint { opacity: 1; }
  .near .drop-hint { opacity: 0.85; outline: 2px dashed rgb(242 240 234 / 0.7); outline-offset: -10px; }
  .over .drop-hint { opacity: 1; background: rgb(20 19 17 / 0.75); outline: 2px solid #f2f0ea; outline-offset: -10px; }
  .replaced { margin: 0; color: var(--ink-2); font-size: 12px; }
  .screen { position: absolute; inset: 0; overflow: hidden; border-radius: 3px; background: #f2f0ea; }
  .eink { filter: grayscale(1) contrast(1.12); }
  .typo { display: flex; }
  .typo :global(.cover) { border-radius: 0; height: 100%; aspect-ratio: auto; }

  .pills { display: flex; flex-wrap: wrap; justify-content: center; gap: 6px; }
  .pills button { min-height: 36px; padding: 0 14px; border-radius: 999px; background: transparent; border-color: var(--rule-strong); }
  .pills button.on { background: var(--ink); color: var(--on-accent); border-color: var(--ink); }
  .note { margin: 0; text-align: center; max-width: 360px; color: var(--ink-2); }
  .hint { margin: 0; color: var(--ink-3); font-size: 12px; display: flex; align-items: center; gap: 6px; text-align: center; max-width: 380px; }
  .details .hint { text-align: left; max-width: none; margin-top: -8px; }
  .check { display: flex; align-items: center; gap: 8px; min-height: 32px; }
  .small { font-size: 12px; }
  .center { justify-content: center; text-align: center; }
  .zoom { display: flex; align-items: center; gap: 10px; width: 100%; max-width: 320px; font-size: 12px; color: var(--ink-2); }
  .zoom input { flex: 1; min-width: 60px; accent-color: var(--ink); }
  .zoom .num { width: 38px; text-align: right; font-variant-numeric: tabular-nums; }
  .seg.small button { display: inline-flex; align-items: center; gap: 6px; }
  .swatch { width: 12px; height: 12px; border-radius: 3px; border: 1px solid var(--rule-strong); }

  .readonly { margin: 0; padding: 10px 12px; border-radius: 8px; background: color-mix(in srgb, #e3b23c 16%, transparent); }
  .field { display: flex; flex-direction: column; gap: 6px; }
  .field > span { color: var(--ink-2); font-size: 12px; }
  .field input { min-height: 40px; font-size: 14px; }
  .title-input { font-family: var(--serif); font-size: 19px !important; min-height: 46px !important; }
  .two { display: grid; grid-template-columns: minmax(0, 1fr); gap: 14px; }
  @container (min-width: 340px) { .two { grid-template-columns: repeat(2, minmax(0, 1fr)); } }

  .stats { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 1px; background: var(--rule); border: 1px solid var(--rule); border-radius: 10px; overflow: hidden; }
  @container (min-width: 440px) { .stats { grid-template-columns: repeat(4, minmax(0, 1fr)); } }
  .stats div { background: var(--sheet); padding: 12px 14px; display: flex; flex-direction: column; gap: 2px; }
  .stats b { font-family: var(--serif); font-size: 22px; font-weight: 600; font-variant-numeric: tabular-nums; }
  .stats span { color: var(--ink-2); font-size: 12px; }

  .actions { margin-top: auto; display: flex; flex-wrap: wrap; gap: 10px; align-items: center; padding-top: 8px; }
  .actions button { min-height: 44px; padding: 0 18px; border-radius: 9px; display: flex; align-items: center; gap: 8px; }
  .grow { flex: 1; }
  .danger-btn { background: transparent; color: var(--danger); }
</style>
