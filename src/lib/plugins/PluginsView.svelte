<script>
  // Installed plugins, installing from ZIP (with the permissions shown first), on/off, removal.
  import { open as openDialog, ask } from "@tauri-apps/plugin-dialog";
  import { api } from "../api.js";
  import { pluginUrl } from "./host.js";

  // plugins: [{manifest, enabled, permissions}]; onChanged(): reload the list; onOpenSettings(id)
  let { plugins, onChanged, onOpenSettings, onError } = $props();

  let pending = $state(null); // {path, manifest, permissions, installedVersion}
  let busy = $state(false);

  async function pick() {
    const path = await openDialog({ multiple: false, filters: [{ name: "X Studio plugin", extensions: ["zip"] }] });
    if (!path) return;
    try {
      pending = { path, ...(await api.pluginInspect(path)) };
    } catch (e) {
      onError(`Not a plugin: ${e}`);
    }
  }

  async function install() {
    busy = true;
    try {
      await api.pluginInstall(pending.path);
      pending = null;
      await onChanged();
    } catch (e) {
      onError(String(e));
    } finally {
      busy = false;
    }
  }

  async function toggle(p) {
    try {
      await api.pluginSetEnabled(p.manifest.id, !p.enabled);
      await onChanged();
    } catch (e) {
      onError(String(e));
    }
  }

  async function remove(p) {
    const ok = await ask(`Remove “${p.manifest.name}”?`, { title: "Remove plugin", kind: "warning", okLabel: "Remove", cancelLabel: "Cancel" });
    if (!ok) return;
    const keepData = await ask("Keep its settings and sign-in so they come back if you install it again?", {
      title: p.manifest.name,
      okLabel: "Keep",
      cancelLabel: "Delete them",
    });
    try {
      await api.pluginRemove(p.manifest.id, keepData);
      await onChanged();
    } catch (e) {
      onError(String(e));
    }
  }

  function contributions(m) {
    const c = m.contributes ?? {};
    const out = [];
    if (c.section) out.push(`“${c.section.title}” in the sidebar`);
    if (c.bookActions?.length) out.push(`${c.bookActions.map((a) => `“${a.title}”`).join(", ")} in the book editor`);
    return out;
  }
</script>

<div class="page">
  <header class="head">
    <div class="titles">
      <h1>Plugins</h1>
      <p>Extra sections, book sources and book commands. Plugins run sandboxed: they reach the network and the reader only as their permissions allow.</p>
    </div>
    <button class="primary" onclick={pick}>Install from ZIP…</button>
  </header>

  {#if pending}
    <div class="consent" role="dialog" aria-label="Install plugin">
      <div class="who">
        <div>
          <p class="big">{pending.manifest.name} <span class="ver">{pending.manifest.version}</span></p>
          <p class="sub">
            {pending.manifest.author || "Unknown author"} · <span class="mono">{pending.manifest.id}</span>
            {#if pending.installedVersion}<br />Replaces version {pending.installedVersion} (settings are kept){/if}
          </p>
        </div>
      </div>
      {#if pending.manifest.description}<p>{pending.manifest.description}</p>{/if}
      <p class="label">It will be able to</p>
      <ul class="perms">
        {#each pending.permissions as line (line.text)}
          <li class:warn={line.warning}>{line.text}</li>
        {/each}
      </ul>
      <p class="label">It adds</p>
      <ul class="perms">
        {#each contributions(pending.manifest) as line (line)}<li>{line}</li>{/each}
      </ul>
      <p class="note">Install plugins only from people you trust.</p>
      <div class="actions">
        <button onclick={() => (pending = null)} disabled={busy}>Cancel</button>
        <button class="primary" onclick={install} disabled={busy}>{busy ? "Installing…" : pending.installedVersion ? "Update" : "Install"}</button>
      </div>
    </div>
  {/if}

  {#if !plugins.length}
    <div class="empty">
      <p class="big">No plugins yet</p>
      <p>Install one from a .zip file.</p>
    </div>
  {:else}
    <ul class="list">
      {#each plugins as p (p.manifest.id)}
        <li class:off={!p.enabled}>
          <span class="icon">
            {#if p.manifest.icon && p.enabled}<img src={pluginUrl(p.manifest.id, p.manifest.icon)} alt="" />{:else}<span>{p.manifest.name.slice(0, 1)}</span>{/if}
          </span>
          <span class="info">
            <span class="name">{p.manifest.name} <span class="ver">{p.manifest.version}</span></span>
            {#if p.manifest.description}<span class="desc">{p.manifest.description}</span>{/if}
            <span class="desc muted">{p.permissions.map((l) => l.text).join(" · ")}</span>
          </span>
          <span class="ctl">
            {#if p.enabled && p.manifest.contributes?.settings}<button class="link" onclick={() => onOpenSettings(p.manifest.id)}>Settings</button>{/if}
            <button class="link" onclick={() => toggle(p)}>{p.enabled ? "Turn off" : "Turn on"}</button>
            <button class="link danger" onclick={() => remove(p)}>Remove</button>
          </span>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .page { padding: 26px 32px 28px; display: flex; flex-direction: column; gap: 20px; }
  .head { display: flex; flex-wrap: wrap; align-items: flex-end; gap: 16px; }
  .titles { flex: 1 1 320px; }
  h1 { margin: 0; font-family: var(--serif); font-weight: 600; font-size: 30px; line-height: 1.1; }
  .titles p { margin: 6px 0 0; color: var(--ink-2); max-width: 60ch; }
  .head .primary { height: 36px; padding: 0 16px; }

  .consent { display: flex; flex-direction: column; gap: 10px; max-width: 560px; padding: 18px 20px; background: var(--sheet); border: 1px solid var(--rule-strong); border-radius: 12px; box-shadow: 0 12px 32px -18px rgb(0 0 0 / 0.4); }
  .consent p { margin: 0; }
  .big { font-family: var(--serif); font-size: 20px; color: var(--ink); }
  .ver { font-family: inherit; font-size: 12px; color: var(--ink-3); font-weight: 400; }
  .sub { color: var(--ink-2); font-size: 12px; }
  .label { margin-top: 4px !important; font-size: 11px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--ink-3); }
  .perms { margin: 0; padding-left: 18px; display: flex; flex-direction: column; gap: 3px; }
  .perms .warn { color: var(--danger); font-weight: 600; }
  .note { color: var(--ink-3); font-size: 12px; }
  .actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 6px; }
  .actions button { height: 34px; padding: 0 16px; }

  .list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; background: var(--sheet); border: 1px solid var(--rule); border-radius: 10px; }
  .list li { display: flex; align-items: center; gap: 14px; padding: 12px 16px; }
  .list li + li { border-top: 1px solid var(--rule-soft); }
  .list li.off .info { opacity: 0.55; }
  .icon { flex: none; width: 36px; height: 36px; border-radius: 8px; background: var(--paper-2); display: grid; place-items: center; overflow: hidden; font-family: var(--serif); font-weight: 600; font-size: 17px; color: var(--ink-2); }
  .icon img { width: 100%; height: 100%; object-fit: contain; }
  .info { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .name { font-weight: 600; }
  .desc { color: var(--ink-2); font-size: 12px; }
  .muted { color: var(--ink-3); }
  .ctl { display: flex; gap: 6px; flex: none; }
  .mono { font-family: var(--mono); font-size: 11px; }
  .empty { padding: 60px 20px; text-align: center; color: var(--ink-2); }
  .empty p { margin: 0 0 6px; }
</style>
