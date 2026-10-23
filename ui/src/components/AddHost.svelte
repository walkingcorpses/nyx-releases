<!-- AddHost.svelte - listens for every way a torrent can arrive and opens the add dialog:
       - Ctrl+V with a magnet link on the clipboard
       - a .torrent file dropped on the window (or magnets/files forwarded by the backend:
         clicking a magnet link, "open with", a second launch)
     The toolbar buttons set `addRequest` directly. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import AddDialog from './AddDialog.svelte';
  import { api } from '../lib/api';
  import { isMagnet } from '../lib/format';
  import { addRequest, dragging } from '../lib/store';
  import type { AddSource } from '../lib/types';

  /** Turns a magnet or a path into an add request (ignores anything else). */
  function toSource(arg: string): AddSource | null {
    if (isMagnet(arg)) return { kind: 'magnet', uri: arg.trim() };
    if (arg.toLowerCase().endsWith('.torrent')) return { kind: 'file', path: arg };
    return null;
  }

  /** Opens the dialog for the first usable argument (one dialog at a time). */
  function openFirst(args: string[]): void {
    const src = args.map(toSource).find((s) => s !== null);
    if (src) addRequest.set(src);
  }

  /** Ctrl+V anywhere outside a text box: a pasted magnet opens the dialog. */
  function onPaste(e: ClipboardEvent): void {
    const target = e.target as HTMLElement | null;
    if (target && ['INPUT', 'TEXTAREA'].includes(target.tagName)) return;
    const text = e.clipboardData?.getData('text') ?? '';
    if (isMagnet(text)) {
      e.preventDefault();
      addRequest.set({ kind: 'magnet', uri: text.trim() });
    }
  }

  onMount(() => {
    const offs: Array<() => void> = [];
    let alive = true;
    // Arguments that arrived before we were listening (cold start with a magnet), then live ones.
    api.takePendingArgs().then((a) => alive && openFirst(a));
    api.onOpen(() => api.takePendingArgs().then(openFirst)).then((f) => (alive ? offs.push(f) : f()));
    api.onDrop(openFirst).then((f) => (alive ? offs.push(f) : f()));
    api.onDragState((over) => dragging.set(over)).then((f) => (alive ? offs.push(f) : f()));
    return () => {
      alive = false;
      offs.forEach((f) => f());
    };
  });
</script>

<svelte:window onpaste={onPaste} />

{#if $addRequest}
  {#key $addRequest}
    <AddDialog source={$addRequest} onclose={() => addRequest.set(null)} />
  {/key}
{/if}
