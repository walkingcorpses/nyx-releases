<!-- CategoryEditor.svelte - named categories, each with a default save folder. Picking a
     category in the add dialog fills in that folder. -->
<script lang="ts">
  import { api } from '../lib/api';
  import type { Category } from '../lib/types';

  let { categories = $bindable() }: { categories: Category[] } = $props();

  let name = $state('');
  let dir = $state('');

  /** Name already used (case-insensitive)? */
  const duplicate = $derived(categories.some((c) => c.name.toLowerCase() === name.trim().toLowerCase()));
  const canAdd = $derived(name.trim() !== '' && dir.trim() !== '' && !duplicate);

  function add(): void {
    categories = [...categories, { name: name.trim(), save_dir: dir.trim() }];
    name = '';
    dir = '';
  }

  function remove(i: number): void {
    categories = categories.filter((_, j) => j !== i);
  }

  async function browse(): Promise<void> {
    const p = await api.pickFolder();
    if (p) dir = p;
  }
</script>

<div class="cats">
  {#each categories as c, i}
    <div class="cat glass">
      <span class="chip">{c.name}</span>
      <input class="field" type="text" bind:value={c.save_dir} aria-label="Folder for {c.name}" />
      <button class="btn icon danger" aria-label="Remove {c.name}" onclick={() => remove(i)}>✕</button>
    </div>
  {:else}
    <p class="muted">No categories yet.</p>
  {/each}

  <div class="new">
    <input class="field" type="text" placeholder="Category name" bind:value={name} aria-label="New category name" />
    <input class="field" type="text" placeholder="Save folder" bind:value={dir} aria-label="New category folder" />
    <button class="btn" onclick={browse}>Browse…</button>
    <button class="btn primary" disabled={!canAdd} onclick={add}>Add</button>
  </div>
  {#if duplicate}<p class="error-text">A category with that name already exists.</p>{/if}
</div>

<style>
  .cats { display: grid; gap: 8px; }
  .cat { display: grid; grid-template-columns: 110px 1fr 30px; gap: 8px; align-items: center; padding: 6px 8px; }
  .new { display: grid; grid-template-columns: 150px 1fr auto auto; gap: 6px; }
</style>
