<!-- FileTree.svelte - the pre-download file picker: files grouped by folder with checkboxes.
     A folder checkbox toggles everything under it and shows a mixed state when only some are on.
     `checked` is bound to the dialog, one boolean per file in torrent order. -->
<script lang="ts">
  import { formatBytes } from '../lib/format';
  import type { FileItem } from '../lib/types';

  let { files, checked = $bindable() }: { files: FileItem[]; checked: boolean[] } = $props();

  interface Node {
    name: string;
    path: string;
    children: Node[];
    index: number | null; // file index for leaves
  }

  /** Builds the folder tree from slash-separated paths. Folders are created on demand. */
  const root = $derived.by(() => {
    const top: Node = { name: '', path: '', children: [], index: null };
    files.forEach((f, i) => {
      let cur = top;
      const parts = f.path.split(/[\\/]/).filter(Boolean);
      parts.forEach((part, depth) => {
        const path = parts.slice(0, depth + 1).join('/');
        const leaf = depth === parts.length - 1;
        let next = cur.children.find((c) => c.name === part && (c.index === null) === !leaf);
        if (!next) {
          next = { name: part, path, children: [], index: leaf ? i : null };
          cur.children.push(next);
        }
        cur = next;
      });
    });
    return top;
  });

  let collapsed = $state(new Set<string>());

  /** File indices under a node (itself, for a leaf). */
  function leaves(n: Node): number[] {
    return n.index !== null ? [n.index] : n.children.flatMap(leaves);
  }

  /** 'all' | 'some' | 'none' for a node's checkboxes. */
  function checkState(n: Node): 'all' | 'some' | 'none' {
    const l = leaves(n);
    const on = l.filter((i) => checked[i]).length;
    return on === 0 ? 'none' : on === l.length ? 'all' : 'some';
  }

  /** Toggles a node: a mixed or empty folder turns fully on, a full one turns off. */
  function toggle(n: Node): void {
    const turnOn = checkState(n) !== 'all';
    const next = [...checked];
    for (const i of leaves(n)) next[i] = turnOn;
    checked = next;
  }

  /** Total bytes under a node. */
  function size(n: Node): number {
    return leaves(n).reduce((a, i) => a + files[i].size, 0);
  }

  function fold(n: Node): void {
    const s = new Set(collapsed);
    if (s.has(n.path)) s.delete(n.path); else s.add(n.path);
    collapsed = s;
  }
</script>

{#snippet row(n: Node, depth: number)}
  <div class="line" style="padding-left: {depth * 18 + 6}px">
    {#if n.index === null}
      <button class="fold" aria-label={collapsed.has(n.path) ? 'Expand' : 'Collapse'} onclick={() => fold(n)}>
        {collapsed.has(n.path) ? '▸' : '▾'}
      </button>
    {:else}
      <span class="fold"></span>
    {/if}
    <label>
      <input
        type="checkbox"
        checked={checkState(n) === 'all'}
        indeterminate={checkState(n) === 'some'}
        onchange={() => toggle(n)}
        aria-label={n.name}
      />
      <span class="nm" class:folder={n.index === null}>{n.name}</span>
    </label>
    <span class="sz num muted">{formatBytes(size(n))}</span>
  </div>
  {#if n.index === null && !collapsed.has(n.path)}
    {#each n.children as c (c.path + (c.index ?? 'd'))}
      {@render row(c, depth + 1)}
    {/each}
  {/if}
{/snippet}

<div class="tree" role="tree" aria-label="Files">
  {#each root.children as c (c.path + (c.index ?? 'd'))}
    {@render row(c, 0)}
  {/each}
</div>

<style>
  .tree { max-height: 240px; overflow: auto; border: 1px solid var(--line); border-radius: var(--radius-sm); background: rgba(0, 0, 0, 0.25); padding: 4px 0; }
  .line { display: flex; align-items: center; gap: 4px; height: 26px; padding-right: 10px; }
  .line:hover { background: var(--glass); }
  label { display: flex; align-items: center; gap: 8px; flex: 1; min-width: 0; cursor: pointer; }
  .nm { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .nm.folder { font-weight: 600; }
  .fold { width: 16px; background: none; border: 0; color: var(--text-dim); padding: 0; cursor: pointer; }
  .sz { font-size: 11px; }
  input[type='checkbox'] { accent-color: var(--violet); }
</style>
