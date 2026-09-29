<script lang="ts">
  import { tick } from "svelte";
  import {
    LOG_LEVELS,
    type LogLevel,
    parseLogLevel,
    parseLogEntry,
    countByLevel,
    filterEntriesAdvanced,
  } from "../lib/log-filter";

  let {
    entries = [],
    maxHeight = "300px",
  }: {
    entries?: string[];
    maxHeight?: string;
  } = $props();

  // Active levels default to error, warn, and info to show actionable logs
  let activeLevels = $state<Set<LogLevel>>(new Set(["error", "warn", "info"]));
  let selectedLevel = $state<LogLevel>("info");
  let searchQuery = $state("");
  let copiedId = $state<number | null>(null);
  let copyAllSuccess = $state(false);
  let listEl: HTMLUListElement | undefined = $state();

  let counts = $derived(countByLevel(entries));
  let filteredEntries = $derived(
    filterEntriesAdvanced(entries, activeLevels, searchQuery),
  );

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

  function toggleLevel(level: LogLevel) {
    const next = new Set(activeLevels);
    if (next.has(level)) {
      next.delete(level);
    } else {
      next.add(level);
    }
    activeLevels = next;
  }

  function selectAllLevels() {
    activeLevels = new Set(LOG_LEVELS);
  }

  function selectWarnAndInfo() {
    activeLevels = new Set(["warn", "info", "error"]);
  }

  function onSelectChange(e: Event) {
    const val = (e.target as HTMLSelectElement).value as LogLevel;
    selectedLevel = val;
    const minPriority = LOG_LEVELS.indexOf(val);
    activeLevels = new Set(LOG_LEVELS.filter((_, idx) => idx >= minPriority));
  }

  async function copyToClipboard(text: string, id: number) {
    try {
      await navigator.clipboard.writeText(text);
      copiedId = id;
      setTimeout(() => {
        if (copiedId === id) copiedId = null;
      }, 1500);
    } catch {
      // Fallback
    }
  }

  async function copyAllFiltered() {
    try {
      await navigator.clipboard.writeText(filteredEntries.join("\n"));
      copyAllSuccess = true;
      setTimeout(() => {
        copyAllSuccess = false;
      }, 1500);
    } catch {
      // Fallback
    }
  }

  function levelBadgeStyle(level: LogLevel, active: boolean): string {
    if (!active) {
      return "opacity-45 hover:opacity-80 bg-surface-1 text-text-muted border-border";
    }
    switch (level) {
      case "error":
        return "bg-status-err-bg text-status-err border-status-err-border font-bold";
      case "warn":
        return "bg-status-warn-bg text-status-warn border-status-warn-border font-bold";
      case "info":
        return "bg-accent-bg text-accent border-border-strong font-semibold";
      case "debug":
        return "bg-surface-2 text-text-secondary border-border font-medium";
      case "trace":
        return "bg-surface-2 text-text-muted border-border font-medium";
    }
  }

  function levelRowColor(level: LogLevel): string {
    switch (level) {
      case "error":
        return "text-status-err bg-status-err-bg/15";
      case "warn":
        return "text-status-warn bg-status-warn-bg/15";
      case "debug":
      case "trace":
        return "text-text-muted";
      default:
        return "text-text-primary";
    }
  }
</script>

<section
  data-testid="logs-section"
  class="flex flex-col bg-surface-0 border border-border rounded shadow-xs {maxHeight === 'none' ? 'h-full' : ''}"
>
  <div class="px-4 py-2.5 border-b border-border shrink-0 flex flex-col gap-2">
    <div class="flex items-center justify-between gap-3 flex-wrap">
      <div class="flex items-center gap-2">
        <h2 class="text-sm font-semibold text-text-primary m-0">Logs</h2>
        <span class="text-xs text-text-muted">
          {filteredEntries.length} / {entries.length}
        </span>
      </div>

      <div class="flex items-center gap-2 flex-1 justify-end min-w-[240px]">
        <input
          type="search"
          data-testid="log-search-input"
          placeholder="Filter logs (IP, ID, error...)..."
          class="text-xs bg-surface-1 border border-border rounded px-2.5 py-1 text-text-primary placeholder:text-text-muted w-full max-w-[280px] focus:outline-none focus:border-accent"
          bind:value={searchQuery}
        />

        {#if filteredEntries.length > 0}
          <button
            type="button"
            data-testid="log-copy-all-btn"
            title="Copy filtered logs to clipboard"
            onclick={copyAllFiltered}
            class="text-xs px-2 py-1 rounded bg-surface-1 hover:bg-surface-2 border border-border text-text-secondary whitespace-nowrap transition-colors"
          >
            {copyAllSuccess ? "Copied!" : "Copy all"}
          </button>
        {/if}
      </div>
    </div>

    <div class="flex items-center justify-between gap-2 flex-wrap text-xs">
      <div class="flex items-center gap-1.5 flex-wrap">
        <span class="text-text-muted text-[11px] font-medium mr-0.5">Levels:</span>
        {#each LOG_LEVELS as level}
          {@const active = activeLevels.has(level)}
          <button
            type="button"
            data-testid={`log-filter-${level}`}
            onclick={() => toggleLevel(level)}
            class="px-2 py-0.5 rounded text-[11px] border flex items-center gap-1.5 transition-all cursor-pointer {levelBadgeStyle(level, active)}"
            title={`Toggle ${level.toUpperCase()} logs`}
          >
            <span>{level.toUpperCase()}</span>
            <span
              class="text-[10px] px-1 py-0.2 rounded-full {active ? 'bg-surface-0/60' : 'bg-surface-2'}"
            >
              {counts[level]}
            </span>
          </button>
        {/each}
      </div>

      <div class="flex items-center gap-1.5 text-[11px]">
        <button
          type="button"
          onclick={selectWarnAndInfo}
          class="text-text-muted hover:text-text-primary px-1.5 py-0.5 rounded hover:bg-surface-1 transition-colors cursor-pointer"
        >
          Warn+Info
        </button>
        <span class="text-border">|</span>
        <button
          type="button"
          onclick={selectAllLevels}
          class="text-text-muted hover:text-text-primary px-1.5 py-0.5 rounded hover:bg-surface-1 transition-colors cursor-pointer"
        >
          All
        </button>

        <label class="sr-only">
          Minimum Level
          <select
            data-testid="log-level-select"
            bind:value={selectedLevel}
            onchange={onSelectChange}
          >
            {#each LOG_LEVELS as level}
              <option value={level}>{level.toUpperCase()}</option>
            {/each}
          </select>
        </label>
      </div>
    </div>
  </div>

  {#if filteredEntries.length === 0}
    <div class="px-4 py-8 text-center flex flex-col items-center justify-center gap-1">
      <p class="text-sm text-text-muted m-0">No log entries matching filter.</p>
      {#if searchQuery || activeLevels.size < LOG_LEVELS.length}
        <button
          type="button"
          onclick={() => { searchQuery = ""; selectAllLevels(); }}
          class="text-xs text-accent hover:underline mt-1 cursor-pointer"
        >
          Reset filters
        </button>
      {/if}
    </div>
  {:else}
    <ul
      bind:this={listEl}
      data-testid="log-list"
      class="font-mono text-xs overflow-y-auto list-none p-0 m-0 divide-y divide-border {maxHeight === 'none' ? 'flex-1 min-h-0' : ''}"
      style={maxHeight !== 'none' ? `max-height: ${maxHeight}` : ''}
    >
      {#each filteredEntries as entry, idx}
        {@const parsed = parseLogEntry(entry)}
        <li
          class="group px-3 py-1 flex items-start gap-2 hover:bg-surface-1/60 transition-colors {levelRowColor(parsed.level)}"
        >
          <span class="text-text-muted shrink-0 select-all font-mono text-[11px] leading-5">
            {parsed.timestamp}
          </span>
          <span
            class="px-1.5 py-0.5 rounded text-[10px] font-semibold uppercase shrink-0 border {levelBadgeStyle(parsed.level, true)}"
          >
            {parsed.level}
          </span>
          {#if parsed.sourceTag}
            <span
              class="px-1 py-0.2 rounded text-[10px] font-mono font-medium bg-surface-2 text-text-secondary border border-border shrink-0 leading-tight"
            >
              [{parsed.sourceTag}]
            </span>
          {/if}
          <span class="break-all select-text font-mono flex-1 leading-5 text-[11px]">
            {parsed.message}
          </span>
          <button
            type="button"
            title="Copy log entry"
            onclick={() => copyToClipboard(entry, idx)}
            class="opacity-0 group-hover:opacity-100 transition-opacity text-[10px] px-1.5 py-0.5 rounded bg-surface-2 hover:bg-surface-3 text-text-secondary border border-border shrink-0 cursor-pointer"
          >
            {copiedId === idx ? "Copied!" : "Copy"}
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</section>
