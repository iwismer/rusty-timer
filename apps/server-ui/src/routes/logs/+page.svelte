<script lang="ts">
  import { logsStore } from "$lib/stores";
  import { LogViewer } from "@rusty-timer/shared-ui";
</script>

<main class="max-w-[900px] mx-auto px-8 py-6">
  <h1 class="text-xl font-bold text-text-primary mb-6">Logs</h1>
  <LogViewer entries={$logsStore} maxHeight="calc(100vh - 200px)" />
</main>
