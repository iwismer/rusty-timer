<script lang="ts">
  import { tick } from "svelte";
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
  let listEl: HTMLUListElement | undefined = $state();

  let filteredEntries = $derived(filterEntries(entries, selectedLevel));

  let prevCount = 0;

  $effect(() => {
    const count = filteredEntries.length;
    const added = count - prevCount;
    if (added > 0 && listEl) {
      const wasAtTop = listEl.scrollTop < 8;
      const oldScrollTop = listEl.scrollTop;
      const oldScrollHeight = listEl.scrollHeight;
      tick().then(() => {
        if (!listEl) return;
        if (wasAtTop) {
          listEl.scrollTop = 0;
        } else {
          const heightDiff = listEl.scrollHeight - oldScrollHeight;
          listEl.scrollTop = oldScrollTop + heightDiff;
        }
      });
    }
    prevCount = count;
  });

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

<section data-testid="logs-section" class="flex flex-col {maxHeight === 'none' ? 'h-full' : ''}">
  <div
    class="flex items-center justify-between px-4 py-2 border-b border-border shrink-0"
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
      bind:this={listEl}
      class="font-mono text-xs overflow-y-auto list-none p-0 m-0 {maxHeight === 'none' ? 'flex-1 min-h-0' : ''}"
      style={maxHeight !== 'none' ? `max-height: ${maxHeight}` : ''}
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
