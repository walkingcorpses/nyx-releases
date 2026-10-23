<!-- SearchDialog.svelte - built-in search through the provider set in Settings > Automation (an
     RSS/Torznab feed URL with {query}). Nyx ships with no provider: you point it at one you trust.
     Choosing a result hands it to the normal add dialog (file tree, folder, category). -->
<script lang="ts">
  import { api } from '../lib/api';
  import { formatBytes } from '../lib/format';
  import { addRequest, searchOpen } from '../lib/store';
  import type { SearchResult } from '../lib/types';

  let query = $state('');
  let results = $state<SearchResult[] | null>(null);
  let busy = $state(false);
  let error = $state('');

  function close(): void {
    searchOpen.set(false);
  }

  /** Runs the search; shows the backend's message if there is no provider or it fails. */
  async function run(): Promise<void> {
    if (!query.trim() || busy) return;
    busy = true;
    error = '';
    try {
      results = await api.search(query.trim());
    } catch (e) {
      results = null;
      error = String(e);
    }
    busy = false;
  }

  /** Opens the add dialog for a result. A .torrent link is downloaded to a temp file first. */
  async function pick(r: SearchResult): Promise<void> {
    error = '';
    try {
      if (r.link.toLowerCase().startsWith('magnet:')) {
        addRequest.set({ kind: 'magnet', uri: r.link });
      } else {
        addRequest.set({ kind: 'file', path: await api.fetchTorrentUrl(r.link) });
      }
      close();
    } catch (e) {
      error = String(e);
    }
  }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && close()} />

<div class="scrim" role="presentation">
  <div class="dialog glass" role="dialog" aria-modal="true" aria-label="Search torrents">
    <h2>Search</h2>
    <div class="row">
      <!-- svelte-ignore a11y_autofocus -->
      <input class="field" type="text" placeholder="What are you looking for?" bind:value={query} autofocus
        onkeydown={(e) => e.key === 'Enter' && run()} aria-label="Search query" />
      <button class="btn primary" disabled={!query.trim() || busy} onclick={run}>{busy ? 'Searching…' : 'Search'}</button>
    </div>
    {#if error}<p class="error-text" role="alert">{error}</p>{/if}

    {#if results}
      <div class="results" role="list">
        {#each results as r}
          <div class="res" role="listitem">
            <span class="t" title={r.title}>{r.title}</span>
            <span class="num muted">{r.size ? formatBytes(r.size) : '—'}</span>
            <span class="num seeds">{r.seeders ?? '—'}</span>
            <button class="btn" onclick={() => pick(r)}>Add</button>
          </div>
        {:else}
          <p class="muted pad">Nothing found.</p>
        {/each}
      </div>
    {/if}
    <div class="actions"><button class="btn" onclick={close}>Close</button></div>
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 36px 0 0 0; background: rgba(3, 2, 8, 0.6); display: grid; place-items: center; z-index: 52; backdrop-filter: blur(3px); }
  .dialog { width: min(680px, calc(100vw - 40px)); max-height: calc(100vh - 90px); padding: 20px; background: var(--bg-raised); display: grid; gap: 12px; grid-template-rows: auto auto auto 1fr auto; box-shadow: 0 30px 80px rgba(0, 0, 0, 0.6); }
  h2 { margin: 0; font-size: 16px; }
  .row { display: flex; gap: 8px; }
  .results { overflow: auto; max-height: 360px; border: 1px solid var(--line); border-radius: var(--radius-sm); background: rgba(0, 0, 0, 0.25); }
  .res { display: grid; grid-template-columns: 1fr 80px 50px auto; gap: 10px; align-items: center; padding: 6px 10px; border-bottom: 1px solid rgba(255, 255, 255, 0.04); }
  .res:hover { background: var(--glass); }
  .t { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .seeds { color: var(--good); text-align: right; }
  .pad { padding: 14px; margin: 0; }
  .actions { display: flex; justify-content: flex-end; }
</style>
