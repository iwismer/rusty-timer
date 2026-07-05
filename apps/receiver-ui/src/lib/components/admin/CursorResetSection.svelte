<script lang="ts">
  import { HelpTip, buttonClass } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";
  import {
    streamKey,
    streamLabel,
    type AdminActions,
  } from "$lib/admin-actions.svelte";

  const btnWarn = buttonClass("warn", "xs");

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
    Reset resume cursors per stream. The selected stream will replay from the
    beginning on next connect.
  </p>
  {#if actions.streams.length === 0}
    <p class="text-xs text-text-muted m-0">No streams available.</p>
  {:else}
    <table class="w-full text-sm">
      <thead>
        <tr class="border-b border-border text-left text-text-muted">
          <th class="py-1.5 pr-3 font-medium text-xs">Stream</th>
          <th class="py-1.5 pr-3 font-medium text-xs">
            Epoch
            <HelpTip
              fieldKey="stream_cursor"
              sectionKey="cursor_reset"
              context="receiver-admin"
              onOpenModal={openHelp}
            />
          </th>
          <th class="py-1.5 pr-3 font-medium text-xs">Seq</th>
          <th class="py-1.5 font-medium text-xs"></th>
        </tr>
      </thead>
      <tbody>
        {#each actions.streams as stream (streamKey(stream))}
          {@const key = streamKey(stream)}
          <tr class="border-b border-border/50">
            <td class="py-1.5 pr-3 text-text-primary text-xs">
              {streamLabel(stream)}
              <span class="block text-text-muted font-mono"
                >{stream.reader_ip}</span
              >
            </td>
            <td class="py-1.5 pr-3 text-text-muted tabular-nums text-xs"
              >{stream.cursor_epoch ?? "\u2014"}</td
            >
            <td class="py-1.5 pr-3 text-text-muted tabular-nums text-xs"
              >{stream.cursor_seq ?? "\u2014"}</td
            >
            <td class="py-1.5 text-right">
              <button
                onclick={() => actions.resetCursor(stream)}
                disabled={actions.inFlightKeys.has(key)}
                class={btnWarn}
                aria-label={"Reset cursor for " + streamLabel(stream)}
              >
                {actions.inFlightKeys.has(key)
                  ? "Resetting..."
                  : "Reset Cursor"}
              </button>
              <HelpTip
                fieldKey="reset_cursor"
                sectionKey="cursor_reset"
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
            () => api.resetAllCursors(),
            "Reset all cursors",
            "reset-all-cursors",
          )}
        disabled={actions.inFlightAction === "reset-all-cursors"}
        class={btnWarn}
      >
        {actions.inFlightAction === "reset-all-cursors"
          ? "Resetting..."
          : "Reset All Cursors"}
      </button>
      <HelpTip
        fieldKey="reset_all_cursors"
        sectionKey="cursor_reset"
        context="receiver-admin"
        onOpenModal={openHelp}
      />
    </div>
  {/if}
{:else}
  <p class="text-sm text-text-muted m-0 mb-4">
    Reset resume cursors per stream. The selected stream will replay from the
    beginning on next connect.
  </p>

  {#if actions.streams.length === 0}
    <p class="text-sm text-text-muted m-0">No streams available.</p>
  {:else}
    <table class="w-full text-sm">
      <thead>
        <tr class="border-b border-border text-left text-text-muted">
          <th class="py-2 pr-4 font-medium">Stream</th>
          <th class="py-2 pr-4 font-medium">Forwarder</th>
          <th class="py-2 pr-4 font-medium">Reader</th>
          <th class="py-2 pr-4 font-medium">Epoch</th>
          <th class="py-2 pr-4 font-medium">Seq</th>
          <th class="py-2 font-medium"></th>
        </tr>
      </thead>
      <tbody>
        {#each actions.streams as stream (streamKey(stream))}
          {@const key = streamKey(stream)}
          <tr class="border-b border-border/50">
            <td class="py-2 pr-4">
              {#if stream.display_alias}
                <span class="text-text-primary font-medium"
                  >{stream.display_alias}</span
                >
                <span class="block text-xs text-text-muted"
                  >{stream.forwarder_id} / {stream.reader_ip}</span
                >
              {:else}
                <span class="text-text-primary"
                  >{stream.forwarder_id} / {stream.reader_ip}</span
                >
              {/if}
            </td>
            <td class="py-2 pr-4 text-text-secondary">{stream.forwarder_id}</td>
            <td class="py-2 pr-4 text-text-secondary">{stream.reader_ip}</td>
            <td class="py-2 pr-4 text-text-secondary tabular-nums"
              >{stream.cursor_epoch ?? "\u2014"}</td
            >
            <td class="py-2 pr-4 text-text-secondary tabular-nums"
              >{stream.cursor_seq ?? "\u2014"}</td
            >
            <td class="py-2 text-right">
              <button
                onclick={() => actions.resetCursor(stream)}
                disabled={actions.inFlightKeys.has(key)}
                class="px-2.5 py-1 text-xs font-medium rounded-md text-status-warn border border-status-warn-border bg-status-warn-bg cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
                aria-label={"Reset cursor for " + streamLabel(stream)}
              >
                {actions.inFlightKeys.has(key)
                  ? "Resetting..."
                  : "Reset Cursor"}
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
            () => api.resetAllCursors(),
            "Reset all cursors",
            "reset-all-cursors",
          )}
        disabled={actions.inFlightAction === "reset-all-cursors"}
        class="px-3 py-1.5 text-xs font-medium rounded-md text-status-warn border border-status-warn-border bg-status-warn-bg cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
      >
        {actions.inFlightAction === "reset-all-cursors"
          ? "Resetting..."
          : "Reset All Cursors"}
      </button>
    </div>
  {/if}
{/if}
