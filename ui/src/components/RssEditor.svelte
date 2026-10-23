<!-- RssEditor.svelte - RSS feeds that add torrents automatically. Each feed has an address, a
     title filter (case-insensitive regular expression, empty = everything), an optional category
     and an on/off switch. A new feed is only baselined: items already in it are never downloaded. -->
<script lang="ts">
  import type { Category, Feed } from '../lib/types';

  let { feeds = $bindable(), categories, interval = $bindable() }: {
    feeds: Feed[];
    categories: Category[];
    interval: number;
  } = $props();

  let draft = $state<Feed>({ name: '', url: '', filter: '', category: null, enabled: true });

  /** Why a feed cannot be saved, or null. */
  function problem(f: Feed): string | null {
    if (!/^https?:\/\//i.test(f.url.trim())) return 'The address must start with http:// or https://';
    try {
      if (f.filter.trim()) new RegExp(f.filter, 'i');
    } catch {
      return 'The filter is not a valid regular expression.';
    }
    return null;
  }

  const draftProblem = $derived(draft.url.trim() ? problem(draft) : null);
  const canAdd = $derived(draft.name.trim() !== '' && draft.url.trim() !== '' && !draftProblem);

  function add(): void {
    feeds = [...feeds, { ...draft, name: draft.name.trim(), url: draft.url.trim() }];
    draft = { name: '', url: '', filter: '', category: null, enabled: true };
  }

  function remove(i: number): void {
    feeds = feeds.filter((_, j) => j !== i);
  }
</script>

<div class="rss">
  <label class="inline">Check every
    <input class="field num-in" type="number" min="5" bind:value={interval} aria-label="RSS interval minutes" /> minutes
  </label>

  {#each feeds as f, i}
    {@const err = problem(f)}
    <div class="feed glass" class:bad={err}>
      <input type="checkbox" bind:checked={f.enabled} aria-label="Enable {f.name}" />
      <input class="field" type="text" bind:value={f.name} aria-label="Feed name" />
      <input class="field wide" type="text" bind:value={f.url} aria-label="Feed address for {f.name}" />
      <input class="field" type="text" bind:value={f.filter} placeholder="filter (regex)" aria-label="Filter for {f.name}" />
      <select class="field" bind:value={f.category} aria-label="Category for {f.name}">
        <option value={null}>No category</option>
        {#each categories as c}<option value={c.name}>{c.name}</option>{/each}
      </select>
      <button class="btn icon danger" aria-label="Remove {f.name}" onclick={() => remove(i)}>✕</button>
      {#if err}<p class="error-text full" role="alert">{err}</p>{/if}
    </div>
  {:else}
    <p class="muted">No feeds yet.</p>
  {/each}

  <div class="feed new">
    <span></span>
    <input class="field" type="text" placeholder="Name" bind:value={draft.name} aria-label="New feed name" />
    <input class="field wide" type="text" placeholder="https://…/feed.xml" bind:value={draft.url} aria-label="New feed address" />
    <input class="field" type="text" placeholder="filter (regex)" bind:value={draft.filter} aria-label="New feed filter" />
    <select class="field" bind:value={draft.category} aria-label="New feed category">
      <option value={null}>No category</option>
      {#each categories as c}<option value={c.name}>{c.name}</option>{/each}
    </select>
    <button class="btn primary" disabled={!canAdd} onclick={add}>Add</button>
    {#if draftProblem}<p class="error-text full" role="alert">{draftProblem}</p>{/if}
  </div>
  <p class="muted small">The first time a feed is polled its current items are only recorded, never downloaded; after that, new items whose title matches the filter are added to the category's folder and started.</p>
</div>

<style>
  .rss { display: grid; gap: 8px; }
  .feed { display: grid; grid-template-columns: 20px 92px 2.2fr 1fr 104px auto; gap: 6px; align-items: center; padding: 6px 8px; }
  .feed.new { background: none; border: 0; padding: 0; }
  .feed.bad { border-color: rgba(251, 113, 133, 0.5); }
  .full { grid-column: 1 / -1; margin: 0; }
  .inline { display: flex; align-items: center; gap: 8px; color: var(--text-dim); }
  .num-in { width: 72px; }
  .small { font-size: 12px; margin: 0; }
  input[type='checkbox'] { accent-color: var(--violet); }
</style>
