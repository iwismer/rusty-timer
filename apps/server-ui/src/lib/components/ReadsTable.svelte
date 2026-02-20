<script lang="ts">
  import type { ReadEntry, DedupMode } from "$lib/api";

  interface Props {
    reads: ReadEntry[];
    total: number;
    loading: boolean;
    showStreamColumn?: boolean;
    dedup: DedupMode;
    windowSecs: number;
    limit: number;
    offset: number;
    onParamsChange: () => void;
  }

  let {
    reads,
    total,
    loading,
    showStreamColumn = false,
    dedup = $bindable(),
    windowSecs = $bindable(),
    limit = $bindable(),
    offset = $bindable(),
    onParamsChange,
  }: Props = $props();

  const WINDOW_OPTIONS = [
    { label: "5s", value: 5 },
    { label: "10s", value: 10 },
    { label: "30s", value: 30 },
    { label: "1m", value: 60 },
    { label: "5m", value: 300 },
    { label: "10m", value: 600 },
  ];

  let totalPages = $derived(Math.max(1, Math.ceil(total / limit)));
  let currentPage = $derived(Math.floor(offset / limit) + 1);

  function setDedup(mode: DedupMode) {
    dedup = mode;
    offset = 0;
    onParamsChange();
  }

  function setWindow(secs: number) {
    windowSecs = secs;
    offset = 0;
    onParamsChange();
  }

  function prevPage() {
    if (offset >= limit) {
      offset -= limit;
      onParamsChange();
    }
  }

  function nextPage() {
    if (offset + limit < total) {
      offset += limit;
      onParamsChange();
    }
  }

  function formatParticipant(read: ReadEntry): string {
    if (read.first_name && read.last_name && read.bib != null) {
      return `${read.first_name} ${read.last_name} (#${read.bib})`;
    }
    if (read.bib != null) {
      return `#${read.bib} — Unknown Participant`;
    }
    if (read.tag_id) {
      return `Chip ${read.tag_id}`;
    }
    return "Unknown";
  }
</script>

<div class="space-y-3">
  <!-- Dedup toggle bar -->
  <div class="flex items-center gap-3 flex-wrap">
    <div class="flex rounded-md border border-border overflow-hidden">
      <button
        class="px-3 py-1 text-xs font-medium {dedup === 'none'
          ? 'bg-accent text-white'
          : 'bg-surface-0 text-text-secondary hover:bg-surface-2'}"
        onclick={() => setDedup("none")}
      >
        No Dedup
      </button>
      <button
        class="px-3 py-1 text-xs font-medium border-l border-border {dedup ===
        'first'
          ? 'bg-accent text-white'
          : 'bg-surface-0 text-text-secondary hover:bg-surface-2'}"
        onclick={() => setDedup("first")}
      >
        First Read
      </button>
      <button
        class="px-3 py-1 text-xs font-medium border-l border-border {dedup ===
        'last'
          ? 'bg-accent text-white'
          : 'bg-surface-0 text-text-secondary hover:bg-surface-2'}"
        onclick={() => setDedup("last")}
      >
        Last Read
      </button>
    </div>

    {#if dedup !== "none"}
      <div class="flex items-center gap-1.5">
        <span class="text-xs text-text-muted">Window:</span>
        <select
          class="text-xs px-2 py-1 rounded-md border border-border bg-surface-0 text-text-primary"
          value={windowSecs}
          onchange={(e) => setWindow(Number(e.currentTarget.value))}
        >
          {#each WINDOW_OPTIONS as opt (opt.value)}
            <option value={opt.value}>{opt.label}</option>
          {/each}
        </select>
      </div>
    {/if}

    <span class="text-xs text-text-muted ml-auto">
      {total.toLocaleString()} reads
    </span>
  </div>

  <!-- Table -->
  {#if loading}
    <p class="text-sm text-text-muted italic">Loading reads…</p>
  {:else if reads.length === 0}
    <p class="text-sm text-text-muted italic">No reads found.</p>
  {:else}
    <div class="overflow-x-auto">
      <table class="w-full text-sm border-collapse">
        <thead>
          <tr class="border-b border-border text-left">
            {#if showStreamColumn}
              <th class="px-2 py-1.5 text-xs font-medium text-text-muted"
                >Stream</th
              >
            {/if}
            <th class="px-2 py-1.5 text-xs font-medium text-text-muted"
              >Participant</th
            >
            <th class="px-2 py-1.5 text-xs font-medium text-text-muted">Bib</th>
            <th class="px-2 py-1.5 text-xs font-medium text-text-muted"
              >Chip ID</th
            >
            <th class="px-2 py-1.5 text-xs font-medium text-text-muted"
              >Timestamp</th
            >
            <th class="px-2 py-1.5 text-xs font-medium text-text-muted"
              >Received</th
            >
          </tr>
        </thead>
        <tbody>
          {#each reads as read, i (read.stream_id + "-" + read.seq)}
            <tr class="border-b border-border/50 hover:bg-surface-1">
              {#if showStreamColumn}
                <td
                  class="px-2 py-1.5 font-mono text-xs text-text-secondary truncate max-w-[120px]"
                >
                  {read.stream_id.slice(0, 8)}…
                </td>
              {/if}
              <td class="px-2 py-1.5 text-text-primary">
                {formatParticipant(read)}
              </td>
              <td class="px-2 py-1.5 font-mono text-text-secondary">
                {read.bib ?? "—"}
              </td>
              <td
                class="px-2 py-1.5 font-mono text-text-secondary text-xs truncate max-w-[140px]"
              >
                {read.tag_id ?? "—"}
              </td>
              <td class="px-2 py-1.5 font-mono text-text-secondary">
                {read.reader_timestamp ?? "—"}
              </td>
              <td class="px-2 py-1.5 text-text-secondary text-xs">
                {new Date(read.received_at).toLocaleString()}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>

    <!-- Pagination -->
    {#if totalPages > 1}
      <div class="flex items-center justify-between pt-2">
        <button
          class="px-3 py-1 text-xs font-medium rounded-md bg-surface-2 border border-border text-text-secondary cursor-pointer hover:bg-surface-3 disabled:opacity-50 disabled:cursor-not-allowed"
          disabled={offset === 0}
          onclick={prevPage}
        >
          Previous
        </button>
        <span class="text-xs text-text-muted">
          Page {currentPage} of {totalPages}
        </span>
        <button
          class="px-3 py-1 text-xs font-medium rounded-md bg-surface-2 border border-border text-text-secondary cursor-pointer hover:bg-surface-3 disabled:opacity-50 disabled:cursor-not-allowed"
          disabled={offset + limit >= total}
          onclick={nextPage}
        >
          Next
        </button>
      </div>
    {/if}
  {/if}
</div>
