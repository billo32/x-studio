<script>
  // A book cover: the real thumbnail when we have it, otherwise a typographic one
  // whose colours are derived from the title so each book keeps its look.
  let { title = "", author = "", thumb = null, small = false } = $props();

  const PALETTES = [
    ["#2f3b35", "#e9e2d0"], ["#7a2e22", "#f1e6d6"], ["#3b4a6b", "#ede7da"], ["#ddd5c4", "#2a2723"],
    ["#1f2a2e", "#d9c9a3"], ["#4c5b3f", "#efe9db"], ["#1e3a4c", "#eae3d2"], ["#b5562e", "#f6eee2"],
    ["#6b4e71", "#f2eaf0"], ["#2a2a2a", "#e6e2d8"], ["#c9b27c", "#2a2416"], ["#5a3d2b", "#f0e4d4"],
  ];
  function hash(s) {
    let h = 2166136261;
    for (const c of s) h = Math.imul(h ^ c.codePointAt(0), 16777619);
    return h >>> 0;
  }
  const palette = $derived(PALETTES[hash(title + author) % PALETTES.length]);
</script>

<div class="cover" class:small style:--bg={palette[0]} style:--fg={palette[1]}>
  {#if thumb}
    <img src={thumb} alt="" draggable="false" />
  {:else}
    <span class="author">{author}</span>
    <span class="rule"></span>
    <span class="title">{title}</span>
  {/if}
</div>

<style>
  .cover {
    position: relative; aspect-ratio: 3 / 5; width: 100%; box-sizing: border-box; overflow: hidden;
    padding: 16px 14px; border-radius: 4px 8px 8px 4px; display: flex; flex-direction: column; justify-content: space-between;
    background: var(--bg); color: var(--fg);
    box-shadow: inset 3px 0 0 rgb(0 0 0 / 0.18), 0 1px 0 var(--rule), 0 10px 22px -14px rgb(0 0 0 / 0.45);
  }
  img { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover; }
  .author { font-size: 9px; letter-spacing: 0.14em; text-transform: uppercase; opacity: 0.8; overflow: hidden; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; }
  .rule { position: absolute; left: 14px; right: 14px; top: 38%; height: 1px; background: currentColor; opacity: 0.35; }
  .title { font-family: var(--serif); font-weight: 600; font-size: 17px; line-height: 1.15; overflow: hidden; display: -webkit-box; -webkit-line-clamp: 5; -webkit-box-orient: vertical; overflow-wrap: anywhere; }
  .small { padding: 0; border-radius: 2px 4px 4px 2px; box-shadow: inset 2px 0 0 rgb(0 0 0 / 0.2); }
  .small .author, .small .rule, .small .title { display: none; }
</style>
