<!-- SettingsPage.svelte - full settings editor in a modal. Tabs: Connection (encryption, proxy),
     Limits (caps, schedule), Queue, Categories, System. Edits a private copy of the settings and
     only hands it back on Save, so Cancel discards everything. -->
<script lang="ts">
  import CategoryEditor from './CategoryEditor.svelte';
  import ScheduleEditor from './ScheduleEditor.svelte';
  import { api } from '../lib/api';
  import { blockError } from '../lib/schedule';
  import type { Settings } from '../lib/types';

  let { settings, onsave, onclose }: {
    settings: Settings;
    onsave: (s: Settings) => Promise<void>;
    onclose: () => void;
  } = $props();

  type Tab = 'connection' | 'limits' | 'queue' | 'categories' | 'system';
  const tabs: { id: Tab; label: string }[] = [
    { id: 'connection', label: 'Connection' },
    { id: 'limits', label: 'Limits' },
    { id: 'queue', label: 'Queue' },
    { id: 'categories', label: 'Categories' },
    { id: 'system', label: 'System' },
  ];
  let tab = $state<Tab>('connection');

  // Private working copy (structuredClone keeps nested arrays independent of the store).
  // svelte-ignore state_referenced_locally
  let draft = $state<Settings>(structuredClone($state.snapshot(settings)));
  // svelte-ignore state_referenced_locally
  let proxyOn = $state(settings.proxy !== null);
  let proxy = $state({
    // svelte-ignore state_referenced_locally
    host: settings.proxy?.host ?? '',
    // svelte-ignore state_referenced_locally
    port: settings.proxy?.port ?? 1080,
    // svelte-ignore state_referenced_locally
    username: settings.proxy?.username ?? '',
    // svelte-ignore state_referenced_locally
    password: settings.proxy?.password ?? '',
  });
  let saving = $state(false);
  let error = $state('');
  let note = $state('');

  /** A proxy needs a host and a valid port when switched on. */
  const proxyError = $derived(
    proxyOn && (proxy.host.trim() === '' || !(proxy.port >= 1 && proxy.port <= 65535))
      ? 'A proxy needs a host and a port (1-65535).'
      : '',
  );
  const scheduleBad = $derived(draft.schedule.blocks.some((b) => blockError(b) !== null));
  const canSave = $derived(!saving && !proxyError && !scheduleBad && draft.max_active_downloads >= 1 && draft.max_active_seeds >= 1);

  /** KB/s <-> bytes/s for the two global caps. */
  const kb = (bytes: number) => Math.round(bytes / 1024);

  async function save(): Promise<void> {
    saving = true;
    error = '';
    const out: Settings = {
      ...$state.snapshot(draft),
      proxy: proxyOn
        ? { host: proxy.host.trim(), port: proxy.port, username: proxy.username || null, password: proxy.password || null }
        : null,
    };
    try {
      await onsave(out);
      onclose();
    } catch (e) {
      error = String(e);
      saving = false;
    }
  }

  /** Registers or removes the .torrent / magnet: handlers for this exe. */
  async function assoc(on: boolean): Promise<void> {
    note = '';
    try {
      await (on ? api.registerAssociations() : api.unregisterAssociations());
      note = on ? 'Nyx is now the default for .torrent files and magnet links.' : 'Nyx default handlers removed.';
    } catch (e) {
      note = String(e);
    }
  }

  async function browseDefault(): Promise<void> {
    const p = await api.pickFolder();
    if (p) draft.default_save_dir = p;
  }
</script>

<div class="scrim" role="presentation">
  <div class="dialog glass" role="dialog" aria-modal="true" aria-label="Settings">
    <aside>
      <h2>Settings</h2>
      {#each tabs as t}
        <button class:on={tab === t.id} role="tab" aria-selected={tab === t.id} onclick={() => (tab = t.id)}>{t.label}</button>
      {/each}
    </aside>

    <div class="content">
      {#if tab === 'connection'}
        <h3>Peer encryption</h3>
        <label class="radio"><input type="radio" name="enc" value="Forced" bind:group={draft.encryption} /> <span><b>Forced</b> <span class="muted">(recommended): refuse peers that do not encrypt</span></span></label>
        <label class="radio"><input type="radio" name="enc" value="Prefer" bind:group={draft.encryption} /> <span><b>Prefer encrypted</b> <span class="muted">: connect to plaintext peers too, for hard-to-seed torrents</span></span></label>
        <p class="notice">
          Message stream encryption (MSE) hides what you transfer and defeats simple traffic-pattern blocking.
          It is obfuscation, not authentication, and it does not hide your IP address from other peers in a swarm.
          Only a VPN or proxy does that.
        </p>

        <h3>SOCKS5 proxy</h3>
        <label class="radio"><input type="checkbox" bind:checked={proxyOn} /> <span>Use a SOCKS5 proxy for all torrent traffic</span></label>
        {#if proxyOn}
          <div class="grid2">
            <label>Host <input class="field" type="text" bind:value={proxy.host} aria-label="Proxy host" /></label>
            <label>Port <input class="field" type="number" min="1" max="65535" bind:value={proxy.port} aria-label="Proxy port" /></label>
            <label>Username <input class="field" type="text" bind:value={proxy.username} aria-label="Proxy username" /></label>
            <label>Password <input class="field" type="password" bind:value={proxy.password} aria-label="Proxy password" /></label>
          </div>
          <p class="muted small">Peers, trackers and hostname lookups go through the proxy. Local peer discovery, UPnP and NAT-PMP are switched off. If the proxy is unreachable, Nyx does not fall back to a direct connection.</p>
          {#if proxyError}<p class="error-text" role="alert">{proxyError}</p>{/if}
        {/if}

      {:else if tab === 'limits'}
        <h3>Global speed limits</h3>
        <div class="grid2">
          <label>Download <span class="muted">(KB/s, 0 = unlimited)</span>
            <input class="field" type="number" min="0" value={kb(draft.down_limit)} oninput={(e) => (draft.down_limit = Math.max(0, Number(e.currentTarget.value) || 0) * 1024)} aria-label="Download limit" />
          </label>
          <label>Upload <span class="muted">(KB/s, 0 = unlimited)</span>
            <input class="field" type="number" min="0" value={kb(draft.up_limit)} oninput={(e) => (draft.up_limit = Math.max(0, Number(e.currentTarget.value) || 0) * 1024)} aria-label="Upload limit" />
          </label>
        </div>
        <h3>Weekly schedule</h3>
        <p class="muted small">A time block replaces the global limits while it is active.</p>
        <ScheduleEditor bind:blocks={draft.schedule.blocks} />

      {:else if tab === 'queue'}
        <h3>Active torrents</h3>
        <div class="grid2">
          <label>Max active downloads <input class="field" type="number" min="1" bind:value={draft.max_active_downloads} aria-label="Max active downloads" /></label>
          <label>Max active seeds <input class="field" type="number" min="1" bind:value={draft.max_active_seeds} aria-label="Max active seeds" /></label>
        </div>
        <p class="muted small">Torrents beyond these limits wait in the queue.</p>

      {:else if tab === 'categories'}
        <h3>Default save folder</h3>
        <div class="row">
          <input class="field" type="text" bind:value={draft.default_save_dir} aria-label="Default save folder" />
          <button class="btn" onclick={browseDefault}>Browse…</button>
        </div>
        <h3>Categories</h3>
        <CategoryEditor bind:categories={draft.categories} />

      {:else}
        <h3>Window</h3>
        <label class="radio"><input type="checkbox" bind:checked={draft.minimize_to_tray} /> <span>Closing the window keeps Nyx running in the tray</span></label>
        <label class="radio"><input type="checkbox" bind:checked={draft.start_minimized} /> <span>Start minimized</span></label>
        <h3>File types</h3>
        <div class="row">
          <button class="btn" onclick={() => assoc(true)}>Make Nyx the default</button>
          <button class="btn" onclick={() => assoc(false)}>Remove default</button>
        </div>
        <p class="muted small">Sets .torrent files and magnet links to open in Nyx, for your Windows account only.</p>
        {#if note}<p class="small">{note}</p>{/if}
      {/if}

      {#if error}<p class="error-text" role="alert">{error}</p>{/if}
      <footer>
        <button class="btn" onclick={onclose}>Cancel</button>
        <button class="btn primary" onclick={save} disabled={!canSave}>Save</button>
      </footer>
    </div>
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 36px 0 0 0; background: rgba(3, 2, 8, 0.6); display: grid; place-items: center; z-index: 60; backdrop-filter: blur(3px); }
  .dialog { width: min(780px, calc(100vw - 40px)); height: min(600px, calc(100vh - 80px)); display: grid; grid-template-columns: 170px 1fr; background: var(--bg-raised); overflow: hidden; box-shadow: 0 30px 80px rgba(0, 0, 0, 0.6); }
  aside { border-right: 1px solid var(--line); padding: 16px 10px; display: flex; flex-direction: column; gap: 2px; }
  aside h2 { margin: 0 8px 12px; font-size: 16px; }
  aside button { background: none; border: 0; text-align: left; padding: 8px 10px; border-radius: 6px; color: var(--text-dim); cursor: pointer; }
  aside button:hover { background: var(--glass); color: var(--text); }
  aside button.on { background: var(--violet-soft); color: #d6c8ff; }
  .content { padding: 18px 22px; overflow-y: auto; display: flex; flex-direction: column; gap: 10px; }
  h3 { margin: 10px 0 0; font-size: 12px; text-transform: uppercase; letter-spacing: 0.08em; color: var(--text-faint); font-weight: 600; }
  .radio { display: flex; align-items: flex-start; gap: 10px; cursor: pointer; }
  .radio input { margin-top: 3px; accent-color: var(--violet); }
  .grid2 { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; }
  .grid2 label { display: grid; gap: 4px; color: var(--text-dim); }
  .row { display: flex; gap: 8px; }
  .small { font-size: 12px; margin: 0; }
  .notice { margin: 4px 0; padding: 10px 12px; border-radius: var(--radius-sm); background: var(--cyan-soft); border: 1px solid rgba(34, 211, 238, 0.25); color: #bdeefa; font-size: 12px; }
  footer { margin-top: auto; padding-top: 12px; display: flex; justify-content: flex-end; gap: 8px; }
</style>
