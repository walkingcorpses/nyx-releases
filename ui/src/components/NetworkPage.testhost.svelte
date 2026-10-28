<!-- NetworkPage.testhost.svelte - test-only wrapper: holds the settings draft as reactive $state the way
     SettingsPage does, so NetworkPage.test.ts can check how the page reacts to edits. Never imported by the app. -->
<script lang="ts">
  import NetworkPage from './NetworkPage.svelte';
  import type { Settings } from '../lib/types';

  let { initial }: { initial: Settings } = $props();
  // svelte-ignore state_referenced_locally
  let draft = $state<Settings>(structuredClone(initial));
</script>

<NetworkPage bind:draft />
