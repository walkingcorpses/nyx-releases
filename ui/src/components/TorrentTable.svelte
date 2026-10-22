<!-- TorrentTable.svelte - the main view: a compact, sortable table of torrents. Rows are
     virtualised (only the visible slice is in the DOM) so a thousand torrents scroll smoothly.
     Clicking a row selects it, which opens the detail drawer. -->
<script lang="ts">
  import ProgressBar from './ProgressBar.svelte';
  import { api } from '../lib/api';
  import { formatBytes, formatEta, formatPercent, formatRate } from '../lib/format';
  import { selectedId, sort, visible, type SortKey } from '../lib/store';
  import type { Torrent } from '../lib/types';

  const ROW_H = 46;
  const OVERSCAN = 6;

  let viewport: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  let height = $state(600);

  /** Index range of rows to render, with a little overscan so fast scrolling never shows gaps. */
  const range = $derived.by(() => {
    const first = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
    const count = Math.ceil(height / ROW_H) + OVERSCAN * 2;
    return { first, last: Math.min($visible.length, first + count) };
  });
  const slice = $derived($visible.slice(range.first, range.last));

  const columns: { key: SortKey; label: string; align?: 'right' }[] = [
    { key: 'name', label: 'Name' },
    { key: 'size', label: 'Size', align: 'right' },
    { key: 'progress', label: 'Progress' },
    { key: 'down_rate', label: 'Down', align: 'right' },
    { key: 'up_rate', label: 'Up', align: 'right' },
    { key: 'peers', label: 'Peers', align: 'right' },
    { key: 'status', label: 'Status' },
  ];

  /** Header click: sort by that column, or flip direction if it is already the sort column. */
  function sortBy(key: SortKey): void {
    sort.update((s) => (s.key === key ? { key, dir: s.dir === 1 ? -1 : 1 } : { key, dir: 1 }));
  }

  /** Time left at the current speed, or a dash when it is not downloading. */
  function eta(t: Torrent): string {
    if (t.status !== 'downloading' || t.down_rate <= 0) return '';
    return formatEta((t.size * (1 - t.progress)) / t.down_rate);
  }

  /** Pause or resume from the row button without selecting the row. */
  function toggle(e: MouseEvent, t: Torrent): void {
    e.stopPropagation();
    void api.torrentAction(t.id, t.status === 'paused' ? 'resume' : 'pause');
  }

  /** Human label for the status cell. */
  function label(t: Torrent): string {
    return t.status === 'error' ? 'Error' : t.status[0].toUpperCase() + t.status.slice(1);
  }
</script>

<section class="table glass" aria-label="Torrents">
  <div class="head row">
    {#each columns as c}
      <button class="th" class:right={c.align === 'right'} onclick={() => sortBy(c.key)} aria-label="Sort by {c.label}">
        {c.label}
        {#if $sort.key === c.key}<span class="arrow">{$sort.dir === 1 ? '▲' : '▼'}</span>{/if}
      </button>
    {/each}
    <span></span>
  </div>

  <div
    class="viewport"
    bind:this={viewport}
    bind:clientHeight={height}
    onscroll={() => (scrollTop = viewport?.scrollTop ?? 0)}
  >
    {#if $visible.length === 0}
      <div class="empty">
        <svg viewBox="0 0 24 24" width="44" height="44" aria-hidden="true">
          <defs>
            <linearGradient id="eg" x1="0" y1="0" x2="1" y2="1">
              <stop offset="0" stop-color="#8b5cf6" /><stop offset="1" stop-color="#22d3ee" />
            </linearGradient>
          </defs>
          <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" fill="url(#eg)" opacity="0.85" />
        </svg>
        <h2>Nothing here yet</h2>
        <p class="muted">Drop a <b>.torrent</b> file on this window, paste a magnet link, or press <b>Add</b>.</p>
      </div>
    {:else}
      <div class="spacer" style="height: {$visible.length * ROW_H}px">
        {#each slice as t, i (t.id)}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div
            class="row item"
            class:selected={$selectedId === t.id}
            style="transform: translateY({(range.first + i) * ROW_H}px)"
            role="row"
            tabindex="-1"
            onclick={() => selectedId.set(t.id)}
          >
            <div class="name" title={t.error ?? t.name}>
              <span class="text">{t.name}</span>
              {#if t.category}<span class="chip">{t.category}</span>{/if}
            </div>
            <div class="num right muted">{t.size ? formatBytes(t.size) : '—'}</div>
            <div class="progress">
              <ProgressBar value={t.progress} active={t.status === 'downloading'} error={t.status === 'error'} />
              <span class="num pct muted">{formatPercent(t.progress)}{eta(t) ? ` · ${eta(t)}` : ''}</span>
            </div>
            <div class="num right" class:dim={!t.down_rate}>{t.down_rate ? formatRate(t.down_rate) : '—'}</div>
            <div class="num right" class:dim={!t.up_rate}>{t.up_rate ? formatRate(t.up_rate) : '—'}</div>
            <div class="num right muted">{t.peers || '—'}</div>
            <div class="status {t.status}"><i></i>{label(t)}</div>
            <button class="act" aria-label={t.status === 'paused' ? 'Resume' : 'Pause'} onclick={(e) => toggle(e, t)}>
              {#if t.status === 'paused'}
                <svg viewBox="0 0 10 10" width="10" height="10"><path d="M2 1l7 4-7 4z" fill="currentColor" /></svg>
              {:else}
                <svg viewBox="0 0 10 10" width="10" height="10"><path d="M2 1h2v8H2zM6 1h2v8H6z" fill="currentColor" /></svg>
              {/if}
            </button>
          </div>
        {/each}
      </div>
    {/if}
  </div>
</section>

<style>
  .table { display: flex; flex-direction: column; min-height: 0; flex: 1; overflow: hidden; }
  .row {
    display: grid; align-items: center; column-gap: 12px; padding: 0 14px;
    grid-template-columns: minmax(200px, 1fr) 84px 210px 92px 92px 54px 106px 30px;
  }
  .head { height: 34px; border-bottom: 1px solid var(--line); flex: none; }
  .th {
    background: none; border: 0; padding: 0; text-align: left; cursor: pointer;
    color: var(--text-faint); font-size: 11px; text-transform: uppercase; letter-spacing: 0.07em;
  }
  .th:hover { color: var(--text); }
  .th.right { text-align: right; }
  .arrow { color: var(--cyan); margin-left: 4px; font-size: 8px; }

  .viewport { flex: 1; overflow-y: auto; overflow-x: hidden; position: relative; }
  .spacer { position: relative; }
  .item {
    position: absolute; left: 0; right: 0; top: 0; height: 46px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.04); cursor: default;
    transition: background 0.12s;
  }
  .item:hover { background: var(--glass); }
  .item.selected { background: var(--violet-soft); box-shadow: inset 2px 0 var(--violet); }

  .name { display: flex; align-items: center; gap: 8px; min-width: 0; }
  .name .text { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .right { text-align: right; }
  .dim { color: var(--text-faint); }
  .progress { display: grid; gap: 3px; }
  .pct { font-size: 11px; }

  .status { display: flex; align-items: center; gap: 7px; font-size: 12px; }
  .status i { width: 7px; height: 7px; border-radius: 50%; background: var(--text-faint); }
  .status.downloading i { background: var(--cyan); box-shadow: 0 0 8px var(--cyan); }
  .status.seeding i { background: var(--good); }
  .status.error i { background: var(--bad); }
  .status.queued i, .status.checking i, .status.metadata i { background: var(--warn); }
  .status.error { color: var(--bad); }

  .act {
    width: 24px; height: 24px; border-radius: 50%; border: 1px solid var(--line); background: transparent;
    color: var(--text-dim); display: grid; place-items: center; cursor: pointer; opacity: 0;
    transition: opacity 0.12s, background 0.12s;
  }
  .item:hover .act, .item.selected .act { opacity: 1; }
  .act:hover { background: var(--glass-strong); color: var(--text); }

  .empty { height: 100%; display: grid; place-content: center; justify-items: center; gap: 4px; text-align: center; padding: 20px; }
  .empty h2 { margin: 8px 0 0; font-size: 16px; font-weight: 600; }
  .empty p { margin: 0; }
</style>
