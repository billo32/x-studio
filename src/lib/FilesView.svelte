<script>
  import { formatSize, parentPath, baseName } from "./api.js";

  // Fonts, dictionaries and the rest. `grouped` folds files into their folder
  // (a font family or a dictionary is one folder of files).
  let { files, grouped = false, emptyTitle, emptyText, query, onDownload, onRemove } = $props();

  const rows = $derived.by(() => {
    const q = query.trim().toLowerCase();
    const list = files.filter((f) => !q || f.path.toLowerCase().includes(q));
    if (!grouped) return list.map((f) => ({ key: f.path, name: f.name, where: parentPath(f.path), size: f.size, count: 1, entry: f }));
    const groups = new Map();
    for (const f of list) {
      const dir = parentPath(f.path);
      const isRoot = ["/fonts", "/.fonts", "/dictionaries", "/.dictionaries", "/"].includes(dir.toLowerCase());
      const key = isRoot ? f.path : dir;
      const g = groups.get(key) ?? { key, name: isRoot ? f.name : baseName(dir), where: isRoot ? dir : parentPath(dir), size: 0, count: 0, entry: isRoot ? f : { path: dir, name: baseName(dir), isDir: true } };
      g.size += f.size;
      g.count += 1;
      groups.set(key, g);
    }
    return [...groups.values()].sort((a, b) => a.name.localeCompare(b.name));
  });
</script>

{#if files.length === 0}
  <div class="empty">
    <p class="big">{emptyTitle}</p>
    <p>{emptyText}</p>
  </div>
{:else}
  <div class="table-box">
    <table>
      <thead><tr><th>Name</th><th>Folder</th><th class="num">Size</th><th></th></tr></thead>
      <tbody>
        {#each rows as r (r.key)}
          <tr>
            <td>
              <span class="name">{r.name}</span>
              {#if r.count > 1}<span class="count">{r.count} files</span>{/if}
            </td>
            <td class="mono muted">{r.where}</td>
            <td class="num muted">{formatSize(r.size)}</td>
            <td class="actions">
              {#if !r.entry.isDir}<button class="link" onclick={() => onDownload(r.entry)}>Download</button>{/if}
              <button class="link danger" onclick={() => onRemove(r.entry)}>Delete</button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}

<style>
  .table-box { overflow-x: auto; background: var(--sheet); border: 1px solid var(--rule); border-radius: 10px; }
  table { width: 100%; border-collapse: collapse; min-width: 560px; }
  th { text-align: left; color: var(--ink-3); font-size: 11px; font-weight: 500; text-transform: uppercase; letter-spacing: 0.06em; padding: 10px 16px; }
  td { padding: 9px 16px; border-top: 1px solid var(--rule-soft); }
  .name { font-weight: 500; }
  .count { margin-left: 8px; color: var(--ink-3); font-size: 12px; }
  .num { text-align: right; font-variant-numeric: tabular-nums; }
  .muted { color: var(--ink-2); }
  .actions { width: 1%; white-space: nowrap; text-align: right; }
  .empty { padding: 60px 20px; text-align: center; color: var(--ink-2); }
  .empty p { margin: 0 0 6px; }
  .big { font-family: var(--serif); font-size: 20px; color: var(--ink); }
</style>
