<script>
  import { formatSize } from "./api.js";
  import { imageRole } from "./models.js";

  // images: entries; thumbs: { [path]: Preview }; model: { width, height }
  let { images, thumbs, model, query, onDownload, onRemove } = $props();

  const shown = $derived(
    images.filter((i) => !query.trim() || i.path.toLowerCase().includes(query.trim().toLowerCase())).sort((a, b) => a.path.localeCompare(b.path)),
  );

  function fitNote(img) {
    const t = thumbs[img.path];
    if (!t) return "";
    if (t.width === model.width && t.height === model.height) return "fits the screen exactly";
    return `${t.width}×${t.height}`;
  }
</script>

{#if images.length === 0}
  <div class="empty">
    <p class="big">No sleep screens yet</p>
    <p>Drop pictures here — X Studio fits them to the {model.width}×{model.height} screen, makes them grayscale BMP and puts them in /sleep.</p>
    <p>Then on the reader choose <b>Settings → Display → Sleep Screen → Custom</b>. A random picture shows each time it sleeps.</p>
  </div>
{:else}
  <p class="how">Shown when <b>Settings → Display → Sleep Screen</b> is <b>Custom</b> or <b>Cover + Custom</b>. Boot screens come from /bootscreen.</p>
  <div class="grid" style:--ratio="{model.width} / {model.height}">
    {#each shown as img (img.path)}
      <figure>
        <div class="frame">
          <div class="screen">
            {#if thumbs[img.path]}
              <img src={thumbs[img.path].dataUrl} alt="" draggable="false" />
            {:else}
              <span class="spinner"></span>
            {/if}
          </div>
        </div>
        <figcaption>
          <span class="name" title={img.path}>{img.name}</span>
          <span class="sub">{imageRole(img.path)} · {fitNote(img) || formatSize(img.size)}</span>
          <span class="actions">
            <button class="link" onclick={() => onDownload(img)}>Download</button>
            <button class="link danger" onclick={() => onRemove(img)}>Delete</button>
          </span>
        </figcaption>
      </figure>
    {/each}
  </div>
{/if}

<style>
  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: 24px 20px; }
  figure { margin: 0; display: flex; flex-direction: column; gap: 8px; }
  .frame { padding: 8px 7px 13px; background: #2b2a27; border-radius: 12px; }
  .screen { aspect-ratio: var(--ratio); background: #f2f0ea; border-radius: 2px; overflow: hidden; display: grid; place-items: center; }
  img { width: 100%; height: 100%; object-fit: contain; filter: grayscale(1); }
  figcaption { display: flex; flex-direction: column; gap: 1px; }
  .name { font-weight: 500; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .sub { color: var(--ink-2); font-size: 12px; }
  .actions { display: flex; gap: 4px; margin-left: -4px; opacity: 0; transition: opacity 0.15s; }
  figure:hover .actions, figure:focus-within .actions { opacity: 1; }
  .how { margin: -6px 0 0; color: var(--ink-2); }
  .empty { padding: 60px 20px; text-align: center; color: var(--ink-2); }
  .empty p { margin: 0 0 6px; }
  .big { font-family: var(--serif); font-size: 20px; color: var(--ink); }
</style>
