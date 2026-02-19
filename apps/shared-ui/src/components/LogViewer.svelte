<script lang="ts">
  export let entries: string[] = [];
  export let maxHeight: string = "300px";
</script>

<section data-testid="logs-section">
  <h2>Logs</h2>
  {#if entries.length === 0}
    <p>No log entries.</p>
  {:else}
    <ul class="logs" style="max-height: {maxHeight}">
      {#each entries as entry}
        <li>{entry}</li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .logs {
    font-family: monospace;
    font-size: 0.85em;
    overflow-y: auto;
    list-style: none;
    padding: 0;
    margin: 0;
  }
  .logs li {
    padding: 0.15rem 0;
    border-bottom: 1px solid #f0f0f0;
  }
  h2 {
    margin-top: 0;
  }
</style>
