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
  import { streamKey, type AdminActions } from "$lib/admin-actions.svelte";

  const btnNeutral = buttonClass("secondary", "xs");

  let {
    actions,
    compact = false,
  }: {
    actions: AdminActions;
    /** Embedded-tab density; the comfortable variant is the /admin route. */
    compact?: boolean;
  } = $props();
</script>

{#if compact}
  <p class="text-xs text-text-muted m-0 mb-3">
    Set or clear the local forwarding port per subscription.
  </p>
  {#if actions.subscriptions.length === 0}
    <p class="text-xs text-text-muted m-0">No subscriptions.</p>
  {:else}
    <table class={tableClass}>
      <thead>
        <tr class={tableHeadRowClass}>
          <th class={tableHeaderCellClass(true)}>Forwarder</th>
          <th class={tableHeaderCellClass(true)}>Reader</th>
          <th class={tableHeaderCellClass(true)}>Port</th>
          <th class={tableHeaderCellClass(true, "!pr-0")}></th>
        </tr>
      </thead>
      <tbody>
        {#each actions.subscriptions as sub (streamKey(sub))}
          {@const portKey = `port-${streamKey(sub)}`}
          <tr class={tableRowClass}>
            <td class={tableCellClass(true, "text-text-muted")}
              >{sub.forwarder_id}</td
            >
            <td class={tableCellClass(true, "text-text-muted")}
              >{sub.reader_ip}</td
            >
            <td class={tableCellClass(true, "!text-sm")}>
              <input
                type="text"
                inputmode="numeric"
                placeholder="default"
                value={actions.getPortDisplayValue(sub)}
                oninput={(e) =>
                  actions.handlePortInput(
                    sub,
                    (e.target as HTMLInputElement).value,
                  )}
                class="w-20 px-2 py-0.5 text-xs rounded border border-border bg-surface-0 text-text-primary font-mono"
              />
            </td>
            <td class={tableCellClass(true, "!pr-0 !text-sm text-right")}>
              <button
                onclick={() => actions.savePort(sub)}
                disabled={!actions.isPortDirty(sub) ||
                  actions.inFlightKeys.has(portKey)}
                class={btnNeutral}
              >
                {actions.inFlightKeys.has(portKey) ? "Saving..." : "Save"}
              </button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
{:else}
  <p class="text-sm text-text-muted m-0 mb-4">
    Set or clear the local forwarding port per subscription. Leave empty to use
    the default port.
  </p>

  {#if actions.subscriptions.length === 0}
    <p class="text-sm text-text-muted m-0">No subscriptions.</p>
  {:else}
    <table class={tableClass}>
      <thead>
        <tr class={tableHeadRowClass}>
          <th class={tableHeaderCellClass()}>Forwarder</th>
          <th class={tableHeaderCellClass()}>Reader</th>
          <th class={tableHeaderCellClass()}
            >Port Override <HelpTip
              fieldKey="port_override"
              sectionKey="port_overrides"
              context="receiver-admin"
            /></th
          >
          <th class={tableHeaderCellClass(false, "!pr-0")}></th>
        </tr>
      </thead>
      <tbody>
        {#each actions.subscriptions as sub (streamKey(sub))}
          {@const portKey = `port-${streamKey(sub)}`}
          <tr class={tableRowClass}>
            <td class={tableCellClass(false, "text-text-secondary")}
              >{sub.forwarder_id}</td
            >
            <td class={tableCellClass(false, "text-text-secondary")}
              >{sub.reader_ip}</td
            >
            <td class={tableCellClass()}>
              <input
                type="text"
                inputmode="numeric"
                placeholder="default"
                value={actions.getPortDisplayValue(sub)}
                oninput={(e) =>
                  actions.handlePortInput(
                    sub,
                    (e.target as HTMLInputElement).value,
                  )}
                class="w-24 px-2 py-1 text-sm rounded border border-border bg-surface-0 text-text-primary"
              />
            </td>
            <td class={tableCellClass(false, "!pr-0 text-right")}>
              <button
                onclick={() => actions.savePort(sub)}
                disabled={!actions.isPortDirty(sub) ||
                  actions.inFlightKeys.has(portKey)}
                class={btnNeutral}
              >
                {actions.inFlightKeys.has(portKey) ? "Saving..." : "Save"}
              </button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
{/if}
