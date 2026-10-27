<script>
  import Cover from "./Cover.svelte";
  import { formatSize, formatNumber, parentPath } from "./api.js";
  import { bookFormat } from "./models.js";

  // books: entries; cards: { [path]: BookCard }; view: "grid" | "list"
  let { books, cards, view, query, onOpen } = $props();

  let format = $state("all");

  function titleOf(b) {
    return cards[b.path]?.title || b.name.replace(/\.(fb2\.zip|epub|fb2|txt|md|xtch?)$/i, "");
  }
  function authorOf(b) {
    return cards[b.path]?.authors?.join(", ") || "";
  }

  const formats = $derived.by(() => {
    const counts = {};
    for (const b of books) counts[bookFormat(b.name)] = (counts[bookFormat(b.name)] || 0) + 1;
    return Object.entries(counts).sort((a, b) => b[1] - a[1]);
  });

  const shown = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return books
      .filter((b) => format === "all" || bookFormat(b.name) === format)
      .filter((b) => !q || titleOf(b).toLowerCase().includes(q) || authorOf(b).toLowerCase().includes(q) || b.path.toLowerCase().includes(q))
      .sort((a, b) => titleOf(a).localeCompare(titleOf(b)));
  });
</script>

{#if formats.length > 1}
  <div class="chips" role="group" aria-label="Format">
    <button class:on={format === "all"} onclick={() => (format = "all")}>All · {books.length}</button>
    {#each formats as [f, n] (f)}
      <button class:on={format === f} onclick={() => (format = f)}>{f} · {n}</button>
    {/each}
  </div>
{/if}

{#if books.length === 0}
  <div class="empty">
    <p class="big">No books on the reader yet</p>
    <p>Drop EPUB, FB2 or TXT files onto the window — they go to /Books.</p>
  </div>
{:else if shown.length === 0}
  <div class="empty"><p>Nothing found.</p></div>
{:else if view === "grid"}
  <div class="grid">
    {#each shown as b (b.path)}
      <button class="card" onclick={() => onOpen(b)} title={b.path}>
        <Cover title={titleOf(b)} author={authorOf(b)} thumb={cards[b.path]?.thumb} />
        <span class="meta">
          <span class="t">{titleOf(b)}</span>
          <span class="a">{authorOf(b) || (cards[b.path] ? "Unknown author" : " ")}</span>
          <span class="badges">
            <span class="fmt">{bookFormat(b.name)}</span>
            {#if cards[b.path]}<span class="pages">{formatNumber(cards[b.path].pages)} pages</span>{/if}
          </span>
        </span>
      </button>
    {/each}
  </div>
{:else}
  <div class="table-box">
    <table>
      <thead>
        <tr><th>Book</th><th>Format</th><th class="num">Pages</th><th class="num">Size</th><th>Folder</th></tr>
      </thead>
      <tbody>
        {#each shown as b (b.path)}
          <tr onclick={() => onOpen(b)}>
            <td>
              <button class="row-book" onclick={(e) => (e.stopPropagation(), onOpen(b))}>
                <span class="thumb"><Cover small title={titleOf(b)} author={authorOf(b)} thumb={cards[b.path]?.thumb} /></span>
                <span class="who"><span class="t">{titleOf(b)}</span><span class="a">{authorOf(b)}</span></span>
              </button>
            </td>
            <td><span class="fmt">{bookFormat(b.name)}</span></td>
            <td class="num">{cards[b.path] ? formatNumber(cards[b.path].pages) : "…"}</td>
            <td class="num muted">{formatSize(b.size)}</td>
            <td class="mono muted">{parentPath(b.path)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}

<style>
  .chips { display: flex; flex-wrap: wrap; gap: 6px; }
  .chips button { min-height: 30px; padding: 0 12px; border-radius: 999px; font-size: 12px; background: transparent; color: var(--ink-2); border-color: var(--rule-strong); }
  .chips button.on { background: var(--ink); color: var(--on-accent); border-color: var(--ink); }

  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(148px, 1fr)); gap: 26px 22px; }
  .card { display: flex; flex-direction: column; gap: 8px; padding: 0; border: none; background: none; text-align: left; border-radius: 8px; }
  .card:hover :global(.cover) { transform: translateY(-2px); }
  .card :global(.cover) { transition: transform 0.15s; }
  .card:focus-visible { outline-offset: 4px; }
  .meta { display: flex; flex-direction: column; gap: 2px; }
  .t { font-weight: 500; line-height: 1.3; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
  .a { color: var(--ink-2); font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .badges { display: flex; gap: 6px; align-items: center; margin-top: 4px; }
  .fmt { font-size: 10.5px; font-weight: 600; letter-spacing: 0.04em; padding: 1px 6px; border: 1px solid var(--rule-strong); border-radius: 4px; color: var(--ink-2); }
  .pages { font-size: 11.5px; color: var(--ink-3); }

  .table-box { overflow-x: auto; background: var(--sheet); border: 1px solid var(--rule); border-radius: 10px; }
  table { width: 100%; border-collapse: collapse; min-width: 640px; }
  th { text-align: left; color: var(--ink-3); font-size: 11px; font-weight: 500; text-transform: uppercase; letter-spacing: 0.06em; padding: 10px 16px; }
  td { padding: 7px 16px; border-top: 1px solid var(--rule-soft); }
  tr:hover td { background: var(--paper); }
  tbody tr { cursor: pointer; }
  .num { text-align: right; font-variant-numeric: tabular-nums; }
  .muted { color: var(--ink-2); }
  .row-book { display: flex; align-items: center; gap: 12px; border: none; background: none; padding: 0; text-align: left; }
  .thumb { width: 26px; flex: none; }
  .who { display: flex; flex-direction: column; }

  .empty { padding: 60px 20px; text-align: center; color: var(--ink-2); }
  .empty p { margin: 0 0 6px; }
  .big { font-family: var(--serif); font-size: 20px; color: var(--ink); }
</style>
