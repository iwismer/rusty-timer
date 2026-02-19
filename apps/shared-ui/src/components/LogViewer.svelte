<script lang="ts">
  export let entries: string[] = [];
  export let maxHeight: string = "300px";
</script>

<section data-testid="logs-section">
  <div
    class="flex items-center justify-between px-4 py-2 border-b border-border"
  >
    <h2 class="text-sm font-semibold text-text-primary m-0">Logs</h2>
    <span class="text-xs text-text-muted">{entries.length} entries</span>
  </div>
  {#if entries.length === 0}
    <p class="px-4 py-6 text-sm text-text-muted text-center m-0">
      No log entries.
    </p>
  {:else}
    <ul
      class="font-mono text-xs overflow-y-auto list-none p-0 m-0"
      style="max-height: {maxHeight}"
    >
      {#each entries as entry}
        <li class="px-4 py-1 border-b border-border text-text-secondary">
          {entry}
        </li>
      {/each}
    </ul>
  {/if}
</section>
