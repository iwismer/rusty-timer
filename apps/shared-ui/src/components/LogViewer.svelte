<script lang="ts">
  import {
    LOG_LEVELS,
    type LogLevel,
    parseLogLevel,
    filterEntries,
  } from "../lib/log-filter";

  let {
    entries = [],
    maxHeight = "300px",
  }: {
    entries?: string[];
    maxHeight?: string;
  } = $props();

  let selectedLevel = $state<LogLevel>("info");

  let filteredEntries = $derived(filterEntries(entries, selectedLevel));

  function levelColor(level: LogLevel): string {
    switch (level) {
      case "error":
        return "text-status-err";
      case "warn":
        return "text-status-warn";
      case "debug":
      case "trace":
        return "text-text-muted";
      default:
        return "text-text-secondary";
    }
  }
</script>

<section data-testid="logs-section">
  <div
    class="flex items-center justify-between px-4 py-2 border-b border-border"
  >
    <h2 class="text-sm font-semibold text-text-primary m-0">Logs</h2>
    <div class="flex items-center gap-3">
      <label class="flex items-center gap-1.5 text-xs text-text-muted">
        Level
        <select
          data-testid="log-level-select"
          class="text-xs bg-surface-0 border border-border rounded px-1.5 py-0.5 text-text-primary"
          bind:value={selectedLevel}
        >
          {#each LOG_LEVELS as level}
            <option value={level}>{level.toUpperCase()}</option>
          {/each}
        </select>
      </label>
      <span class="text-xs text-text-muted">
        {filteredEntries.length} / {entries.length}
      </span>
    </div>
  </div>
  {#if filteredEntries.length === 0}
    <p class="px-4 py-6 text-sm text-text-muted text-center m-0">
      No log entries.
    </p>
  {:else}
    <ul
      class="font-mono text-xs overflow-y-auto list-none p-0 m-0"
      style="max-height: {maxHeight}"
    >
      {#each filteredEntries as entry}
        <li
          class="px-4 py-1 border-b border-border {levelColor(parseLogLevel(entry))}"
        >
          {entry}
        </li>
      {/each}
    </ul>
  {/if}
</section>
