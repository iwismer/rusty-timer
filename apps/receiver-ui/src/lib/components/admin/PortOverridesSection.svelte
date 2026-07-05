<script lang="ts">
  import { HelpTip } from "@rusty-timer/shared-ui";
  import { streamKey, type AdminActions } from "$lib/admin-actions.svelte";
  import { btnNeutral } from "./button-classes";

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
    <table class="w-full text-sm">
      <thead>
        <tr class="border-b border-border text-left text-text-muted">
          <th class="py-1.5 pr-3 font-medium text-xs">Forwarder</th>
          <th class="py-1.5 pr-3 font-medium text-xs">Reader</th>
          <th class="py-1.5 pr-3 font-medium text-xs">Port</th>
          <th class="py-1.5 font-medium text-xs"></th>
        </tr>
      </thead>
      <tbody>
        {#each actions.subscriptions as sub (streamKey(sub))}
          {@const portKey = `port-${streamKey(sub)}`}
          <tr class="border-b border-border/50">
            <td class="py-1.5 pr-3 text-text-muted text-xs"
              >{sub.forwarder_id}</td
            >
            <td class="py-1.5 pr-3 text-text-muted text-xs">{sub.reader_ip}</td>
            <td class="py-1.5 pr-3">
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
            <td class="py-1.5 text-right">
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
    <table class="w-full text-sm">
      <thead>
        <tr class="border-b border-border text-left text-text-muted">
          <th class="py-2 pr-4 font-medium">Forwarder</th>
          <th class="py-2 pr-4 font-medium">Reader</th>
          <th class="py-2 pr-4 font-medium"
            >Port Override <HelpTip
              fieldKey="port_override"
              sectionKey="port_overrides"
              context="receiver-admin"
            /></th
          >
          <th class="py-2 font-medium"></th>
        </tr>
      </thead>
      <tbody>
        {#each actions.subscriptions as sub (streamKey(sub))}
          {@const portKey = `port-${streamKey(sub)}`}
          <tr class="border-b border-border/50">
            <td class="py-2 pr-4 text-text-secondary">{sub.forwarder_id}</td>
            <td class="py-2 pr-4 text-text-secondary">{sub.reader_ip}</td>
            <td class="py-2 pr-4">
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
            <td class="py-2 text-right">
              <button
                onclick={() => actions.savePort(sub)}
                disabled={!actions.isPortDirty(sub) ||
                  actions.inFlightKeys.has(portKey)}
                class="px-2.5 py-1 text-xs font-medium rounded-md text-text-primary border border-border bg-surface-2 cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
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
