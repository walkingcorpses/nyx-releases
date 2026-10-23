<!-- Toolbar.svelte - add buttons, actions for the selected torrent, status filter chips,
     search and the global speed readout. All actions go through api.ts. -->
<script lang="ts">
  import LimitDialog from './LimitDialog.svelte';
  import { api } from '../lib/api';
  import { addRequest, filter, search, searchOpen, selected, selectedId, settingsOpen, type Filter } from '../lib/store';

  const chips: { id: Filter; label: string }[] = [
    { id: 'all', label: 'All' },
    { id: 'downloading', label: 'Active' },
    { id: 'seeding', label: 'Seeding' },
    { id: 'paused', label: 'Paused' },
    { id: 'error', label: 'Errors' },
  ];

  let removeMenu = $state(false);
  let moreMenu = $state(false);
  let copied = $state(false);
  let limitOpen = $state(false);

  /** File picker -> add dialog. */
  async function addFile(): Promise<void> {
    const path = await api.pickTorrentFile();
    if (path) addRequest.set({ kind: 'file', path });
  }

  /** Runs an action on the selected torrent. */
  function act(action: string): void {
    if (!$selected) return;
    void api.torrentAction($selected.id, action);
    if (action.startsWith('remove')) selectedId.set(null);
    removeMenu = false;
    moreMenu = false;
  }

  /** Re-orders the download queue: position 0 starts first, a huge position goes last. */
  function queueMove(position: number): void {
    if ($selected) void api.moveInQueue($selected.id, position);
    moreMenu = false;
  }

  /** Copies the selected torrent's magnet link; briefly shows a confirmation. */
  async function copyMagnet(): Promise<void> {
    if (!$selected) return;
    await navigator.clipboard.writeText(await api.getMagnet($selected.id));
    copied = true;
    setTimeout(() => (copied = false), 1400);
  }
</script>

<div class="toolbar">
  <div class="group">
    <button class="btn primary" onclick={addFile}>
      <svg viewBox="0 0 10 10" width="10" height="10"><path d="M5 0v10M0 5h10" stroke="currentColor" stroke-width="1.6" /></svg>
      Add
    </button>
    <button class="btn" onclick={() => addRequest.set({ kind: 'paste' })}>Magnet…</button>
    <button class="btn" onclick={() => searchOpen.set(true)}>Search…</button>
  </div>

  <div class="group">
    {#if $selected?.status === 'paused'}
      <button class="btn" onclick={() => act('resume')}>Resume</button>
    {:else}
      <button class="btn" disabled={!$selected} onclick={() => act('pause')}>Pause</button>
    {/if}
    <div class="menu">
      <button class="btn danger" disabled={!$selected} onclick={() => (removeMenu = !removeMenu)}>Remove ▾</button>
      {#if removeMenu}
        <div class="pop glass">
          <button onclick={() => act('remove')}>Remove torrent</button>
          <button class="danger" onclick={() => act('remove_delete')}>Remove and delete files</button>
        </div>
      {/if}
    </div>
    <div class="menu">
      <button class="btn" disabled={!$selected} onclick={() => (moreMenu = !moreMenu)}>{copied ? 'Copied' : 'More ▾'}</button>
      {#if moreMenu}
        <div class="pop glass">
          <button onclick={() => act('open_folder')}>Open folder</button>
          <button onclick={() => { void copyMagnet(); moreMenu = false; }}>Copy magnet link</button>
          <button onclick={() => act('recheck')}>Force recheck</button>
          <button onclick={() => { limitOpen = true; moreMenu = false; }}>Speed limit…</button>
          <button onclick={() => queueMove(0)}>Move to front of queue</button>
          <button onclick={() => queueMove(1_000_000)}>Move to back of queue</button>
        </div>
      {/if}
    </div>
  </div>

  <div class="chips" role="tablist" aria-label="Filter">
    {#each chips as c}
      <button class="chipbtn" class:on={$filter === c.id} onclick={() => filter.set(c.id)}>{c.label}</button>
    {/each}
  </div>

  <input class="field search" type="text" placeholder="Search" bind:value={$search} aria-label="Search torrents" />

  <button class="btn icon" aria-label="Settings" onclick={() => settingsOpen.set(true)}>
    <svg viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.4">
      <circle cx="8" cy="8" r="2.4" />
      <path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M3.4 12.6l1.4-1.4M11.2 4.8l1.4-1.4" />
    </svg>
  </button>
</div>

{#if limitOpen && $selected}
  <LimitDialog torrent={$selected} onclose={() => (limitOpen = false)} />
{/if}

<style>
  .toolbar { display: flex; align-items: center; gap: 14px; padding: 10px 14px; flex-wrap: wrap; }
  .group { display: flex; gap: 6px; }
  .menu { position: relative; }
  .pop { position: absolute; top: 34px; left: 0; z-index: 20; display: grid; padding: 4px; min-width: 190px; background: var(--bg-raised); }
  .pop button { background: none; border: 0; text-align: left; padding: 8px 10px; border-radius: 6px; cursor: pointer; }
  .pop button:hover { background: var(--glass-strong); }
  .pop .danger { color: var(--bad); }
  .chips { display: flex; gap: 2px; padding: 3px; border: 1px solid var(--line); border-radius: 99px; background: rgba(0, 0, 0, 0.25); }
  .chipbtn { border: 0; background: none; padding: 3px 12px; border-radius: 99px; color: var(--text-dim); cursor: pointer; transition: all 0.15s; }
  .chipbtn:hover { color: var(--text); }
  .chipbtn.on { background: var(--violet-soft); color: #d6c8ff; box-shadow: inset 0 0 0 1px rgba(139, 92, 246, 0.4); }
  .search { width: 150px; margin-left: auto; }
</style>
