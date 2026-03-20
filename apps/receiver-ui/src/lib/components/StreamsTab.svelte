<script lang="ts">
  import { AlertBanner } from "@rusty-timer/shared-ui";
  import { resizeWidth } from "$lib/actions/resizeWidth";
  import {
    store,
    streamKey,
    toggleSubscription,
    changeEarliestEpoch,
    replayStream,
    replayAll,
    selectedEarliestEpochValue,
    selectedTargetedEpochValue,
    formatEarliestEpochOption,
    setTargetedEpochInputs,
    markModeEdited,
  } from "$lib/store.svelte";
  import { btnPrimary, btnSecondary } from "$lib/ui-classes";

  let tableWidth = $state(0);
  let expandedKey = $state<string | null>(null);

  // Fixed column widths (approx)
  const READS_COL = 70;
  const PORT_COL = 60;
  const STREAM_COL_MIN = 120;
  const PADDING = 48;

  // Last Read field thresholds
  const TIME_W = 70;
  const BIB_W = 40;
  const NAME_W = 80;
  const CHIP_TRUNC_W = 55;
  const CHIP_FULL_W = 100;

  function availableLastReadWidth(): number {
    return Math.max(
      0,
      tableWidth - STREAM_COL_MIN - READS_COL - PORT_COL - PADDING,
    );
  }

  function showLastReadCol(): boolean {
    return availableLastReadWidth() >= TIME_W;
  }

  function dotClass(online: boolean | undefined | null): string {
    if (online === true) return "bg-status-ok";
    if (online === false) return "bg-status-err";
    return "bg-status-warn";
  }

  function toggleExpand(key: string) {
    expandedKey = expandedKey === key ? null : key;
  }

  function formatLastReadTimestamp(timestamp: string): string {
    const match = timestamp.match(
      /(?:^|[T\s])(\d{2}:\d{2}:\d{2}(?:\.\d+)?)(?:$|Z|[+-]\d{2}:?\d{2}|\s)/,
    );
    return match?.[1] ?? timestamp;
  }

  function formatLastRead(key: string): string {
    const read = store.lastReads.get(key);
    if (!read) return "\u2014";

    const avail = availableLastReadWidth();
    let used = 0;
    const parts: string[] = [];

    // Time (highest priority)
    if (used + TIME_W <= avail) {
      parts.push(formatLastReadTimestamp(read.timestamp));
      used += TIME_W;
    } else {
      return "\u2014";
    }

    // Bib
    if (read.bib && used + BIB_W <= avail) {
      parts.push(`#${read.bib}`);
      used += BIB_W;
    }

    if (read.name) {
      // Has participant name → show name, no chip ID
      if (used + NAME_W <= avail) {
        parts.push(read.name);
        used += NAME_W;
      }
    } else if (read.bib) {
      // Has bib but no name → show "Unknown Participant"
      if (used + NAME_W <= avail) {
        parts.push("Unknown Participant");
        used += NAME_W;
      }
    } else {
      // No bib and no name → show "Unknown Chip <chip-id>"
      const cleaned = read.chip_id.replaceAll(":", "");
      const label = `Unknown Chip ${cleaned}`;
      if (used + NAME_W <= avail) {
        parts.push(label);
        used += NAME_W;
      }
    }

    return parts.join("  ");
  }
</script>

<div class="h-full flex flex-col" use:resizeWidth={(w) => (tableWidth = w)}>
  {#if store.streams?.upstream_error}
    <div class="px-4 py-2">
      <AlertBanner variant="warn" message={store.streams.upstream_error} />
    </div>
  {/if}

  {#if store.modeDraft === "targeted_replay"}
    <div class="flex justify-end px-4 py-2 border-b border-border">
      <button
        data-testid="replay-all-btn"
        class={btnSecondary}
        onclick={() => void replayAll()}
      >
        Replay All
      </button>
    </div>
  {/if}

  {#if !store.streams || store.streams.streams.length === 0}
    <p class="px-4 py-8 text-sm text-text-muted text-center m-0">
      No streams available.
    </p>
  {:else}
    <div class="flex-1 overflow-y-auto">
      <table class="w-full text-sm">
        <thead>
          <tr
            class="sticky top-0 z-10 bg-surface-0 border-b border-border text-left text-text-muted"
          >
            <th class="w-px whitespace-nowrap py-2 px-4 font-medium">Stream</th>
            {#if showLastReadCol()}
              <th class="w-full py-2 px-2 font-medium text-left">Last Read</th>
            {/if}
            <th class="py-2 px-2 font-medium text-right w-[70px]">Reads</th>
            <th class="py-2 px-4 font-medium text-right w-[60px]">Port</th>
          </tr>
        </thead>
        <tbody>
          {#each store.streams.streams as stream (streamKey(stream.forwarder_id, stream.reader_ip))}
            {@const key = streamKey(stream.forwarder_id, stream.reader_ip)}
            <tr
              class="border-b border-border/50 hover:bg-surface-1/50 cursor-pointer"
              role="button"
              tabindex="0"
              onclick={() => toggleExpand(key)}
              onkeydown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  toggleExpand(key);
                }
              }}
            >
              <td class="w-px whitespace-nowrap py-2 px-4">
                <div class="flex items-center gap-2 min-w-0">
                  <span
                    class="w-2.5 h-2.5 rounded-full shrink-0 {dotClass(
                      stream.online,
                    )}"
                  ></span>
                  <span class="text-text-primary">
                    {stream.display_alias ?? stream.forwarder_id}
                  </span>
                </div>
              </td>
              {#if showLastReadCol()}
                <td
                  class="w-full max-w-0 py-2 px-2 text-left text-text-muted font-mono truncate"
                >
                  {formatLastRead(key)}
                </td>
              {/if}
              <td
                class="py-2 px-2 text-right font-mono text-text-primary w-[70px]"
              >
                {stream.subscribed
                  ? (stream.reads_total ?? 0).toLocaleString()
                  : "\u2014"}
              </td>
              <td
                class="py-2 px-4 text-right font-mono text-text-muted w-[60px]"
              >
                {stream.local_port ?? "\u2014"}
              </td>
            </tr>

            {#if expandedKey === key}
              <tr>
                <td colspan={showLastReadCol() ? 4 : 3} class="p-0">
                  <div class="bg-surface-1 px-4 py-3 border-b border-border">
                    <div class="grid grid-cols-2 gap-x-6 gap-y-2 text-xs mb-3">
                      <div>
                        <span class="text-text-muted">Reader IP:</span>
                        <span class="font-mono text-text-primary ml-1"
                          >{stream.reader_ip}</span
                        >
                      </div>
                      <div>
                        <span class="text-text-muted">Forwarder:</span>
                        <span class="font-mono text-text-primary ml-1"
                          >{stream.forwarder_id}</span
                        >
                      </div>
                      {#if stream.stream_epoch !== undefined}
                        <div>
                          <span class="text-text-muted">Epoch:</span>
                          <span class="font-mono text-text-primary ml-1">
                            {stream.stream_epoch}{#if stream.current_epoch_name?.trim()}
                              ({stream.current_epoch_name.trim()}){/if}
                          </span>
                        </div>
                      {/if}
                      {#if stream.subscribed && stream.reads_total !== undefined}
                        <div>
                          <span class="text-text-muted">Reads:</span>
                          <span class="font-mono text-text-primary ml-1">
                            {stream.reads_total} total{#if stream.reads_epoch !== undefined},
                              {stream.reads_epoch} epoch{/if}
                          </span>
                        </div>
                      {/if}
                    </div>

                    <div class="flex items-center gap-2 flex-wrap">
                      {#if store.modeDraft === "targeted_replay"}
                        {@const options = store.earliestEpochOptions[key] ?? []}
                        {@const selectedTargeted =
                          selectedTargetedEpochValue(stream)}
                        <select
                          data-testid="targeted-epoch-{key}"
                          class="px-2 py-1 text-xs rounded font-mono bg-surface-0 border border-border text-text-primary w-36 focus:outline-none focus:ring-1 focus:ring-accent disabled:opacity-50"
                          value={selectedTargeted}
                          onchange={(e) => {
                            e.stopPropagation();
                            setTargetedEpochInputs({
                              ...store.targetedEpochInputs,
                              [key]: e.currentTarget.value,
                            });
                            markModeEdited();
                          }}
                          onclick={(e) => e.stopPropagation()}
                        >
                          {#if store.earliestEpochLoading[key]}
                            <option value="">Loading epochs...</option>
                          {:else if store.earliestEpochLoadErrors[key]}
                            <option value="">Epochs unavailable</option>
                          {:else if options.length === 0}
                            <option value="">No epochs available</option>
                          {:else}
                            {#each options as option}
                              <option value={String(option.stream_epoch)}>
                                {formatEarliestEpochOption(option)}
                              </option>
                            {/each}
                          {/if}
                        </select>
                        <button
                          data-testid="replay-stream-{key}"
                          class="{btnPrimary} !px-2.5 !py-1 !text-xs"
                          onclick={(e) => {
                            e.stopPropagation();
                            void replayStream(stream);
                          }}
                        >
                          Replay
                        </button>
                      {:else}
                        {@const options = store.earliestEpochOptions[key] ?? []}
                        {@const selectedEarliest =
                          selectedEarliestEpochValue(stream)}
                        <select
                          data-testid="earliest-epoch-{key}"
                          class="px-2 py-1 text-xs rounded font-mono bg-surface-0 border border-border text-text-primary w-36 focus:outline-none focus:ring-1 focus:ring-accent disabled:opacity-50"
                          value={selectedEarliest}
                          onchange={(e) => {
                            e.stopPropagation();
                            void changeEarliestEpoch(
                              stream,
                              e.currentTarget.value,
                            );
                          }}
                          onclick={(e) => e.stopPropagation()}
                          disabled={store.modeDraft === "race" ||
                            store.earliestEpochSaving[key]}
                        >
                          {#if store.earliestEpochLoading[key]}
                            <option value="">Loading epochs...</option>
                          {:else if store.earliestEpochLoadErrors[key]}
                            <option value="">Epochs unavailable</option>
                          {:else if options.length === 0}
                            <option value="">No epochs available</option>
                          {:else}
                            {#each options as option}
                              <option value={String(option.stream_epoch)}>
                                {formatEarliestEpochOption(option)}
                              </option>
                            {/each}
                          {/if}
                        </select>
                      {/if}

                      <button
                        data-testid="subscribe-toggle-{key}"
                        class={stream.subscribed ? btnSecondary : btnPrimary}
                        class:!px-2.5={true}
                        class:!py-1={true}
                        class:!text-xs={true}
                        onclick={(e) => {
                          e.stopPropagation();
                          void toggleSubscription(stream);
                        }}
                        disabled={store.streamActionBusy}
                      >
                        {stream.subscribed ? "Unsubscribe" : "Subscribe"}
                      </button>
                    </div>
                  </div>
                </td>
              </tr>
            {/if}
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>
