<script lang="ts">
  import {
    HelpTip,
    buttonClass,
    tableCellClass,
    tableClass,
    tableHeadRowClass,
    tableHeaderCellClass,
    tableRowClass,
  } from "@rusty-timer/shared-ui";
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
    Reset resume cursors per stream. The selected stream will replay from the
    beginning on next connect.
  </p>
  {#if actions.streams.length === 0}
    <p class="text-xs text-text-muted m-0">No streams available.</p>
  {:else}
    <table class={tableClass}>
      <thead>
        <tr class={tableHeadRowClass}>
          <th class={tableHeaderCellClass(true)}>Stream</th>
          <th class={tableHeaderCellClass(true)}>
            Epoch
            <HelpTip
              fieldKey="stream_cursor"
              sectionKey="cursor_reset"
              context="receiver-admin"
              onOpenModal={openHelp}
            />
          </th>
          <th class={tableHeaderCellClass(true)}>Seq</th>
          <th class={tableHeaderCellClass(true, "!pr-0")}></th>
        </tr>
      </thead>
      <tbody>
        {#each actions.streams as stream (streamKey(stream))}
          {@const key = streamKey(stream)}
          <tr class={tableRowClass}>
            <td class={tableCellClass(true, "text-text-primary")}>
              {streamLabel(stream)}
              <span class="block text-text-muted font-mono"
                >{stream.reader_ip}</span
              >
            </td>
            <td class={tableCellClass(true, "text-text-muted tabular-nums")}
              >{stream.cursor_epoch ?? "\u2014"}</td
            >
            <td class={tableCellClass(true, "text-text-muted tabular-nums")}
              >{stream.cursor_seq ?? "\u2014"}</td
            >
            <td class={tableCellClass(true, "!pr-0 !text-sm text-right")}>
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
              <button
                data-testid="reset-stream-data-{key}"
                onclick={() => actions.resetStreamData(stream)}
                disabled={actions.inFlightKeys.has(`stream-data-${key}`)}
                class={btnWarn}
                aria-label={"Reset stream data for " + streamLabel(stream)}
              >
                {actions.inFlightKeys.has(`stream-data-${key}`)
                  ? "Resetting..."
                  : actions.confirmingStreamDataKey === `stream-data-${key}`
                    ? "Confirm Reset Data"
                    : "Reset Stream Data"}
              </button>
              <HelpTip
                fieldKey="reset_stream_data"
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
    <table class={tableClass}>
      <thead>
        <tr class={tableHeadRowClass}>
          <th class={tableHeaderCellClass()}>Stream</th>
          <th class={tableHeaderCellClass()}>Forwarder</th>
          <th class={tableHeaderCellClass()}>Reader</th>
          <th class={tableHeaderCellClass()}>Epoch</th>
          <th class={tableHeaderCellClass()}>Seq</th>
          <th class={tableHeaderCellClass(false, "!pr-0")}></th>
        </tr>
      </thead>
      <tbody>
        {#each actions.streams as stream (streamKey(stream))}
          {@const key = streamKey(stream)}
          <tr class={tableRowClass}>
            <td class={tableCellClass()}>
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
            <td class={tableCellClass(false, "text-text-secondary")}
              >{stream.forwarder_id}</td
            >
            <td class={tableCellClass(false, "text-text-secondary")}
              >{stream.reader_ip}</td
            >
            <td
              class={tableCellClass(false, "text-text-secondary tabular-nums")}
              >{stream.cursor_epoch ?? "\u2014"}</td
            >
            <td
              class={tableCellClass(false, "text-text-secondary tabular-nums")}
              >{stream.cursor_seq ?? "\u2014"}</td
            >
            <td class={tableCellClass(false, "!pr-0 text-right")}>
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
              <button
                data-testid="reset-stream-data-{key}"
                onclick={() => actions.resetStreamData(stream)}
                disabled={actions.inFlightKeys.has(`stream-data-${key}`)}
                class={btnWarn}
                aria-label={"Reset stream data for " + streamLabel(stream)}
              >
                {actions.inFlightKeys.has(`stream-data-${key}`)
                  ? "Resetting..."
                  : actions.confirmingStreamDataKey === `stream-data-${key}`
                    ? "Confirm Reset Data"
                    : "Reset Stream Data"}
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
        class={btnWarnSm}
      >
        {actions.inFlightAction === "reset-all-cursors"
          ? "Resetting..."
          : "Reset All Cursors"}
      </button>
    </div>
  {/if}
{/if}
