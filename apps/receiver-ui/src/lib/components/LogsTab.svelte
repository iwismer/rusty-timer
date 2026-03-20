<script lang="ts">
  import { LogViewer } from "@rusty-timer/shared-ui";
  import { store } from "$lib/store.svelte";
</script>

<div class="h-full">
  <LogViewer entries={store.logEntries} maxHeight="none" />
</div>
