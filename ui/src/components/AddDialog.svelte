<!-- AddDialog.svelte - the pre-download dialog: shows the file tree with checkboxes, the save
     folder, a category and a sequential toggle, then adds the torrent.
       .torrent: inspected locally, added paused on confirm, priorities applied, then resumed.
       magnet:   added immediately so metadata can be fetched ("Fetching metadata…"); nothing
                 downloads until confirm, which sets folder, category, priorities and resumes.
     Cancelling a magnet removes the half-added torrent. -->
<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import FileTree from './FileTree.svelte';
  import { api } from '../lib/api';
  import { formatBytes, isMagnet, magnetName } from '../lib/format';
  import type { AddSource, FileItem, Settings } from '../lib/types';

  let { source, onclose }: { source: AddSource; onclose: () => void } = $props();

  // `source` is fixed for the dialog's lifetime; the paste step swaps in a magnet.
  // svelte-ignore state_referenced_locally
  let current = $state<AddSource>(source);
  let pasted = $state('');
  let name = $state('');
  let files = $state<FileItem[]>([]);
  let checked = $state<boolean[]>([]);
  let folder = $state('');
  let category = $state<string | null>(null);
  let sequential = $state(false);
  let settings = $state<Settings | null>(null);
  let busy = $state(false);
  let error = $state('');
  let magnetId: string | null = null; // set once a magnet has been added for metadata
  let poll: ReturnType<typeof setInterval> | undefined;
  let done = false;

  const fetching = $derived(current.kind === 'magnet' && files.length === 0);
  const canAdd = $derived(!busy && folder.trim() !== '' && !fetching && current.kind !== 'paste');

  onMount(async () => {
    settings = await api.getSettings();
    folder = settings.default_save_dir;
    await begin(current);
  });

  onDestroy(() => clearInterval(poll));

  /** Starts the work for a source: inspect a file, or add a magnet and wait for its metadata. */
  async function begin(src: AddSource): Promise<void> {
    error = '';
    try {
      if (src.kind === 'file') {
        const meta = await api.inspectTorrent(src.path);
        name = meta.name;
        files = meta.files;
        checked = meta.files.map((f) => f.priority !== 0);
      } else if (src.kind === 'magnet') {
        name = magnetName(src.uri);
        magnetId = await api.addMagnet(src.uri, '', null, false);
        poll = setInterval(checkMetadata, 1000);
        void checkMetadata();
      }
    } catch (e) {
      error = String(e);
    }
  }

  /** Asks the backend whether the magnet's file list has arrived yet. */
  async function checkMetadata(): Promise<void> {
    if (!magnetId || files.length > 0) return;
    try {
      const d = await api.getDetail(magnetId);
      if (d.files.length > 0) {
        files = d.files;
        checked = d.files.map(() => true);
        clearInterval(poll);
      }
    } catch {
      /* torrent not ready yet; try again next tick */
    }
  }

  /** Paste step: accepts a magnet link and moves on to the metadata step. */
  async function usePasted(): Promise<void> {
    if (!isMagnet(pasted)) {
      error = 'That does not look like a magnet link.';
      return;
    }
    current = { kind: 'magnet', uri: pasted.trim() };
    await begin(current);
  }

  /** Changing category jumps the folder to the category's own folder. */
  function onCategory(): void {
    const c = settings?.categories.find((x) => x.name === category);
    if (c) folder = c.save_dir;
    else if (settings) folder = settings.default_save_dir;
  }

  async function browse(): Promise<void> {
    const p = await api.pickFolder();
    if (p) folder = p;
  }

  /** Confirms: applies folder, category, file choices and sequential mode, then starts. */
  async function confirm(): Promise<void> {
    busy = true;
    error = '';
    try {
      const prios = checked.map((on) => (on ? 4 : 0));
      let id: string;
      if (current.kind === 'file') {
        id = await api.addTorrent(current.path, folder, category, true);
      } else {
        id = magnetId!;
        await api.setTorrentOptions(id, folder, category);
      }
      await api.setPriorities(id, prios);
      if (sequential) await api.torrentAction(id, 'sequential_on');
      await api.torrentAction(id, 'resume');
      done = true;
      onclose();
    } catch (e) {
      error = String(e);
      busy = false;
    }
  }

  /** Closes without adding. A magnet that was already added for metadata is removed again. */
  async function cancel(): Promise<void> {
    if (magnetId && !done) await api.torrentAction(magnetId, 'remove').catch(() => {});
    onclose();
  }

  const totalSize = $derived(files.reduce((a, f, i) => a + (checked[i] ? f.size : 0), 0));
</script>

<div class="scrim" role="presentation">
  <div class="dialog glass" role="dialog" aria-modal="true" aria-label="Add torrent">
    <h2>Add torrent</h2>

    {#if current.kind === 'paste'}
      <p class="muted">Paste a magnet link:</p>
      <!-- svelte-ignore a11y_autofocus -->
      <input class="field" type="text" placeholder="magnet:?xt=urn:btih:…" bind:value={pasted} autofocus
        onkeydown={(e) => e.key === 'Enter' && usePasted()} aria-label="Magnet link" />
      {#if error}<p class="error-text">{error}</p>{/if}
      <div class="actions">
        <button class="btn" onclick={cancel}>Cancel</button>
        <button class="btn primary" onclick={usePasted} disabled={!pasted.trim()}>Continue</button>
      </div>
    {:else}
      <div class="title" title={name}>{name}</div>

      {#if fetching}
        <p class="fetching muted"><span class="spin"></span> Fetching metadata…</p>
      {:else}
        <FileTree {files} bind:checked />
        <p class="muted small">{checked.filter(Boolean).length} of {files.length} files · {formatBytes(totalSize)}</p>
      {/if}

      <div class="form">
        <label>
          <span>Save to</span>
          <div class="row">
            <input class="field" type="text" bind:value={folder} aria-label="Save folder" />
            <button class="btn" onclick={browse}>Browse…</button>
          </div>
        </label>
        <label>
          <span>Category</span>
          <select class="field" bind:value={category} onchange={onCategory} aria-label="Category">
            <option value={null}>None</option>
            {#each settings?.categories ?? [] as c}<option value={c.name}>{c.name}</option>{/each}
          </select>
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={sequential} aria-label="Sequential download" />
          <span>Download in order (for streaming)</span>
        </label>
      </div>

      {#if error}<p class="error-text">{error}</p>{/if}
      <div class="actions">
        <button class="btn" onclick={cancel}>Cancel</button>
        <button class="btn primary" onclick={confirm} disabled={!canAdd}>Add</button>
      </div>
    {/if}
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 36px 0 0 0; background: rgba(3, 2, 8, 0.6); display: grid; place-items: center; z-index: 50; backdrop-filter: blur(3px); }
  .dialog { width: min(560px, calc(100vw - 40px)); padding: 20px; background: var(--bg-raised); display: grid; gap: 12px; box-shadow: 0 30px 80px rgba(0, 0, 0, 0.6); }
  h2 { margin: 0; font-size: 16px; }
  .title { font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .form { display: grid; gap: 10px; }
  .form label { display: grid; gap: 4px; }
  .form span { color: var(--text-dim); font-size: 12px; }
  .row { display: flex; gap: 6px; }
  .check { display: flex !important; align-items: center; gap: 8px; }
  .check input { accent-color: var(--violet); }
  .actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 4px; }
  .small { font-size: 12px; margin: 0; }
  .fetching { display: flex; align-items: center; gap: 10px; padding: 18px 0; margin: 0; }
  .spin { width: 14px; height: 14px; border-radius: 50%; border: 2px solid var(--line-strong); border-top-color: var(--cyan); animation: spin 0.8s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
</style>
