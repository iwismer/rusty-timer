<script lang="ts">
  import { HelpTip, buttonClass } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";
  import {
    streamKey,
    streamLabel,
    type AdminActions,
  } from "$lib/admin-actions.svelte";

  const btnWarn = buttonClass("warn", "xs");
  const btnWarnSm = buttonClass("warn", "sm");

  let {
    actions,
    compact = false,
    openHelp = undefined,
  }: {
    actions: AdminActions;
    /** Embedded-tab density; the comfortable variant is the /admin route. */
    compact?: boolean;
    /** Store-driven help modal opener (embedded tab only). */
    openHelp?: (fieldKey: string) => void;
  } = $props();
</script>

{#if compact}
  <p class="text-xs text-text-muted m-0 mb-3">
    Clear earliest-epoch overrides per stream or all at once.
  </p>
  {#if actions.streams.length === 0}
    <p class="text-xs text-text-muted m-0">No streams available.</p>
  {:else}
    <table class="w-full text-sm">
      <thead>
        <tr class="border-b border-border text-left text-text-muted">
          <th class="py-1.5 pr-3 font-medium text-xs">Stream</th>
          <th class="py-1.5 font-medium text-xs"></th>
        </tr>
      </thead>
      <tbody>
        {#each actions.streams as stream (streamKey(stream))}
          {@const key = `epoch-${streamKey(stream)}`}
          <tr class="border-b border-border/50">
            <td class="py-1.5 pr-3 text-text-primary text-xs"
              >{streamLabel(stream)}</td
            >
            <td class="py-1.5 text-right">
              <button
                onclick={() => actions.resetEpoch(stream)}
                disabled={actions.inFlightKeys.has(key)}
                class={btnWarn}
                aria-label={"Reset epoch for " + streamLabel(stream)}
              >
                {actions.inFlightKeys.has(key) ? "Resetting..." : "Reset Epoch"}
              </button>
              <HelpTip
                fieldKey="reset_epoch_override"
                sectionKey="epoch_overrides"
                context="receiver-admin"
                onOpenModal={openHelp}
              />
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
    <div class="mt-3 flex justify-end">
      <button
        onclick={() =>
          actions.bulkAction(
            () => api.resetAllEarliestEpochs(),
            "Reset all earliest-epoch overrides",
            "reset-all-epochs",
          )}
        disabled={actions.inFlightAction === "reset-all-epochs"}
        class={btnWarn}
      >
        {actions.inFlightAction === "reset-all-epochs"
          ? "Resetting..."
          : "Reset All Epoch Overrides"}
      </button>
      <HelpTip
        fieldKey="reset_all_epoch_overrides"
        sectionKey="epoch_overrides"
        context="receiver-admin"
        onOpenModal={openHelp}
      />
    </div>
  {/if}
{:else}
  <p class="text-sm text-text-muted m-0 mb-4">
    Clear earliest-epoch overrides per stream or all at once. Streams will
    revert to receiving all available epochs.
  </p>

  {#if actions.streams.length === 0}
    <p class="text-sm text-text-muted m-0">No streams available.</p>
  {:else}
    <table class="w-full text-sm">
      <thead>
        <tr class="border-b border-border text-left text-text-muted">
          <th class="py-2 pr-4 font-medium">Stream</th>
          <th class="py-2 font-medium"></th>
        </tr>
      </thead>
      <tbody>
        {#each actions.streams as stream (streamKey(stream))}
          {@const key = `epoch-${streamKey(stream)}`}
          <tr class="border-b border-border/50">
            <td class="py-2 pr-4">
              <span class="text-text-primary">{streamLabel(stream)}</span>
            </td>
            <td class="py-2 text-right">
              <button
                onclick={() => actions.resetEpoch(stream)}
                disabled={actions.inFlightKeys.has(key)}
                class={btnWarn}
                aria-label={"Reset epoch for " + streamLabel(stream)}
              >
                {actions.inFlightKeys.has(key) ? "Resetting..." : "Reset Epoch"}
              </button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
    <div class="mt-4 flex justify-end">
      <button
        onclick={() =>
          actions.bulkAction(
            () => api.resetAllEarliestEpochs(),
            "Reset all earliest-epoch overrides",
            "reset-all-epochs",
          )}
        disabled={actions.inFlightAction === "reset-all-epochs"}
        class={btnWarnSm}
      >
        {actions.inFlightAction === "reset-all-epochs"
          ? "Resetting..."
          : "Reset All Epoch Overrides"}
      </button>
    </div>
  {/if}
{/if}
