<script lang="ts">
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { store } from "$lib/store.svelte";
  import type { ForwarderConnectionStatus, ServerDeviceStatus } from "$lib/api";
  import {
    connectForwarder,
    disconnectForwarder,
    reconnectForwarder,
  } from "$lib/api";
  import { btnPrimary, btnSecondary } from "$lib/ui-classes";

  type StateDisplay = {
    label: string;
    dotClass: string;
    textClass: string;
  };

  let busyByEndpoint = $state<Record<string, boolean>>({});
  let actionError = $state<string | null>(null);

  function approvalLabel(server: ServerDeviceStatus): string | null {
    if (!server.configured) return "Server not configured";
    if (server.waiting_for_approval) {
      return server.message ?? "Waiting for server approval";
    }
    if (server.reachable === false)
      return server.message ?? "Server unreachable";
    if (server.approval_state === "active") return "Server approved";
    return server.message;
  }

  function approvalClass(server: ServerDeviceStatus): string {
    if (server.waiting_for_approval) return "text-status-warn";
    if (server.reachable === false) return "text-status-err";
    return "text-text-muted";
  }

  function reachableLabel(server: ServerDeviceStatus): string {
    if (!server.configured) return "Not configured";
    if (server.reachable === true) return "Reachable";
    if (server.reachable === false) return "Unreachable";
    return "Reachability unknown";
  }

  function forwarderStateDisplay(
    forwarder: ForwarderConnectionStatus,
  ): StateDisplay {
    if (forwarder.pending) {
      return {
        label: "Connecting…",
        dotClass: "bg-status-warn",
        textClass: "text-status-warn",
      };
    }

    switch (forwarder.state) {
      case "subscribed":
        return {
          label: "Subscribed",
          dotClass: "bg-status-ok",
          textClass: "text-status-ok",
        };
      case "connected":
        return {
          label: "Connected",
          dotClass: "bg-status-ok",
          textClass: "text-status-ok",
        };
      case "unavailable":
        return {
          label: "Unavailable",
          dotClass: "bg-status-err",
          textClass: "text-status-err",
        };
      case "disconnected":
        return {
          label: "Disconnected",
          dotClass: "bg-text-muted",
          textClass: "text-text-muted",
        };
      default:
        return {
          label: "Unknown",
          dotClass: "bg-text-muted",
          textClass: "text-text-muted",
        };
    }
  }

  function adminUrl(): string | null {
    const base = store.savedServerUrl.trim().replace(/\/+$/, "");
    if (base.length === 0) return null;
    return `${base}/admin`;
  }

  async function openAdminPanel(): Promise<void> {
    const url = adminUrl();
    if (!url) return;
    await openUrl(url);
  }

  async function runForwarderAction(
    endpointId: string,
    action: (endpointId: string) => Promise<void>,
  ): Promise<void> {
    busyByEndpoint[endpointId] = true;
    actionError = null;
    try {
      await action(endpointId);
    } catch (e) {
      actionError = String(e);
    } finally {
      busyByEndpoint[endpointId] = false;
    }
  }

  function showConnect(forwarder: ForwarderConnectionStatus): boolean {
    return !forwarder.pending && forwarder.state === "disconnected";
  }

  function showDisconnect(forwarder: ForwarderConnectionStatus): boolean {
    return forwarder.pending || forwarder.state !== "disconnected";
  }

  function showReconnect(forwarder: ForwarderConnectionStatus): boolean {
    return forwarder.pending || forwarder.state !== "disconnected";
  }

  function showReconnectBeforeDisconnect(
    forwarder: ForwarderConnectionStatus,
  ): boolean {
    return !forwarder.pending && forwarder.state === "unavailable";
  }
</script>

<div class="mx-auto max-w-[760px] px-6 py-6">
  {#if store.connections}
    <section
      data-testid="connections-server-card"
      class="rounded-lg border border-border bg-surface-1 p-4"
    >
      <div class="flex items-start justify-between gap-4">
        <div>
          <p class="text-xs font-medium text-text-muted">Server</p>
          <p class="mt-1 text-sm text-text-primary">
            {reachableLabel(store.connections.server)}
          </p>
          {#if store.connections.server.endpoint_id}
            <p class="mt-1 font-mono text-xs text-text-muted">
              {store.connections.server.endpoint_id}
            </p>
          {/if}
          {#if approvalLabel(store.connections.server)}
            <p
              data-testid="server-approval-state"
              class="mt-2 text-xs {approvalClass(store.connections.server)}"
            >
              {approvalLabel(store.connections.server)}
            </p>
          {/if}
        </div>

        {#if store.connections.server.configured}
          <button
            data-testid="open-admin-panel-btn"
            class={btnSecondary}
            onclick={() => void openAdminPanel()}
            disabled={!adminUrl()}
          >
            Open admin panel
          </button>
        {/if}
      </div>
    </section>

    <section class="mt-4 rounded-lg border border-border bg-surface-1">
      <div class="border-b border-border px-4 py-3">
        <p class="text-xs font-medium text-text-muted">Forwarders</p>
      </div>

      {#if store.connections.forwarders.length === 0}
        <p class="px-4 py-6 text-sm text-text-muted">
          No forwarders are available yet.
        </p>
      {:else}
        <div class="divide-y divide-border">
          {#each store.connections.forwarders as forwarder (forwarder.endpoint_id)}
            {@const stateDisplay = forwarderStateDisplay(forwarder)}
            <div
              data-testid={`forwarder-row-${forwarder.endpoint_id}`}
              class="flex items-center justify-between gap-4 px-4 py-3"
            >
              <div class="min-w-0">
                <div class="flex items-center gap-2">
                  <span
                    data-testid={`forwarder-state-${forwarder.endpoint_id}`}
                    class="flex items-center gap-2 text-sm font-medium {stateDisplay.textClass}"
                  >
                    <span class="h-2 w-2 rounded-full {stateDisplay.dotClass}"
                    ></span>
                    {stateDisplay.label}
                  </span>
                  <span class="text-xs text-text-muted">
                    {forwarder.subscribed_count} subscribed / {forwarder.available_count}
                    available
                  </span>
                </div>
                <p class="mt-1 truncate text-sm font-medium text-text-primary">
                  {forwarder.display_name ?? forwarder.endpoint_id}
                </p>
                <p class="mt-0.5 truncate font-mono text-xs text-text-muted">
                  {forwarder.endpoint_id}
                </p>
              </div>

              <div class="flex shrink-0 items-center gap-2">
                {#if showConnect(forwarder)}
                  <button
                    data-testid={`forwarder-connect-${forwarder.endpoint_id}`}
                    class={btnPrimary}
                    onclick={() =>
                      void runForwarderAction(
                        forwarder.endpoint_id,
                        connectForwarder,
                      )}
                    disabled={busyByEndpoint[forwarder.endpoint_id]}
                  >
                    Connect
                  </button>
                {/if}
                {#if showReconnectBeforeDisconnect(forwarder)}
                  <button
                    data-testid={`forwarder-reconnect-${forwarder.endpoint_id}`}
                    class={btnSecondary}
                    onclick={() =>
                      void runForwarderAction(
                        forwarder.endpoint_id,
                        reconnectForwarder,
                      )}
                    disabled={busyByEndpoint[forwarder.endpoint_id]}
                  >
                    Reconnect
                  </button>
                {/if}
                {#if showDisconnect(forwarder)}
                  <button
                    data-testid={`forwarder-disconnect-${forwarder.endpoint_id}`}
                    class={btnSecondary}
                    onclick={() =>
                      void runForwarderAction(
                        forwarder.endpoint_id,
                        disconnectForwarder,
                      )}
                    disabled={busyByEndpoint[forwarder.endpoint_id]}
                  >
                    Disconnect
                  </button>
                {/if}
                {#if showReconnect(forwarder) && !showReconnectBeforeDisconnect(forwarder)}
                  <button
                    data-testid={`forwarder-reconnect-${forwarder.endpoint_id}`}
                    class={btnSecondary}
                    onclick={() =>
                      void runForwarderAction(
                        forwarder.endpoint_id,
                        reconnectForwarder,
                      )}
                    disabled={busyByEndpoint[forwarder.endpoint_id]}
                  >
                    Reconnect
                  </button>
                {/if}
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </section>

    {#if actionError}
      <p class="mt-3 text-xs text-status-err">{actionError}</p>
    {/if}
  {:else}
    <section
      data-testid="connections-server-card"
      class="rounded-lg border border-border bg-surface-1 p-4"
    >
      <p class="text-sm text-text-muted">Loading connections…</p>
    </section>
  {/if}
</div>
