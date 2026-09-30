<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import {
    AlertBanner,
    Card,
    StatCard,
    StatusBadge,
    approvalBadgeState,
    buttonClass,
  } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";
  import type {
    DeviceRecord,
    ForwarderStreamRecord,
    StatusResponse,
  } from "$lib/api";

  let status = $state<StatusResponse | null>(null);
  let error = $state<string | null>(null);
  let actionSuccess = $state<string | null>(null);
  let actionError = $state<string | null>(null);
  let loading = $state(true);
  let busyEndpoint = $state<string | null>(null);
  let copiedId = $state<string | null>(null);
  let copyTimeout: ReturnType<typeof setTimeout> | undefined;
  let nowMs = $state(Date.now());
  let poll: ReturnType<typeof setInterval> | undefined;
  let clockTicker: ReturnType<typeof setInterval> | undefined;

  interface ForwarderViewModel {
    endpoint_id: string;
    display_name: string;
    approval_state: "pending" | "active";
    last_seen_unix_ms: number | null;
    streams: ForwarderStreamRecord[];
  }

  interface ReceiverViewModel {
    endpoint_id: string;
    display_name: string;
    approval_state: "pending" | "active";
    last_seen_unix_ms: number | null;
  }

  // Combine forwarder records from devices and status.forwarders
  let forwarders = $derived.by((): ForwarderViewModel[] => {
    if (!status) return [];
    const fwdMetaByEndpoint = new Map(
      status.forwarders.map((f) => [f.endpoint_id, f]),
    );

    // Streams by forwarder endpoint
    const streamsByEndpoint = new Map<string, ForwarderStreamRecord[]>();
    for (const stream of status.forwarder_streams) {
      const list = streamsByEndpoint.get(stream.endpoint_id) ?? [];
      list.push(stream);
      streamsByEndpoint.set(stream.endpoint_id, list);
    }

    // Devices registered as forwarder
    const forwarderDevices = status.devices.filter(
      (d) => d.device_kind === "forwarder",
    );

    const result: ForwarderViewModel[] = [];
    const seen = new Set<string>();

    for (const dev of forwarderDevices) {
      seen.add(dev.endpoint_id);
      const meta = fwdMetaByEndpoint.get(dev.endpoint_id);
      result.push({
        endpoint_id: dev.endpoint_id,
        display_name:
          dev.display_name?.trim() ||
          meta?.display_name?.trim() ||
          "Unnamed forwarder",
        approval_state: dev.approval_state,
        last_seen_unix_ms:
          dev.last_seen_unix_ms ?? meta?.last_seen_unix_ms ?? null,
        streams: streamsByEndpoint.get(dev.endpoint_id) ?? [],
      });
    }

    // Include any forwarder record not yet in devices table (if any)
    for (const fwd of status.forwarders) {
      if (!seen.has(fwd.endpoint_id)) {
        result.push({
          endpoint_id: fwd.endpoint_id,
          display_name: fwd.display_name?.trim() || "Unnamed forwarder",
          approval_state: fwd.approval_state,
          last_seen_unix_ms: fwd.last_seen_unix_ms,
          streams: streamsByEndpoint.get(fwd.endpoint_id) ?? [],
        });
      }
    }

    return result.sort((a, b) => a.display_name.localeCompare(b.display_name));
  });

  // Filter receiver devices
  let receivers = $derived.by((): ReceiverViewModel[] => {
    if (!status) return [];
    return status.devices
      .filter((d) => d.device_kind === "receiver")
      .map((d) => ({
        endpoint_id: d.endpoint_id,
        display_name: d.display_name?.trim() || "Unnamed receiver",
        approval_state: d.approval_state,
        last_seen_unix_ms: d.last_seen_unix_ms ?? null,
      }))
      .sort((a, b) => a.display_name.localeCompare(b.display_name));
  });

  interface Liveness {
    state: "ok" | "warn" | "err";
    label: string;
    detail: string;
    online: boolean;
  }

  function getLiveness(lastSeenUnixMs: number | null | undefined): Liveness {
    if (!lastSeenUnixMs || lastSeenUnixMs <= 0) {
      return {
        state: "err",
        label: "Offline",
        detail: "Never seen",
        online: false,
      };
    }

    const diffSec = Math.max(0, Math.floor((nowMs - lastSeenUnixMs) / 1000));

    if (diffSec < 60) {
      return {
        state: "ok",
        label: "Online",
        detail: `Seen ${formatRelative(diffSec)} ago`,
        online: true,
      };
    }
    if (diffSec < 180) {
      return {
        state: "warn",
        label: "Stale",
        detail: `Seen ${formatRelative(diffSec)} ago`,
        online: false,
      };
    }
    return {
      state: "err",
      label: "Offline",
      detail: `Last seen ${formatRelative(diffSec)} ago`,
      online: false,
    };
  }

  function formatRelative(diffSec: number): string {
    if (diffSec < 60) return `${diffSec}s`;
    const mins = Math.floor(diffSec / 60);
    if (mins < 60) return `${mins}m`;
    const hours = Math.floor(mins / 60);
    if (hours < 24) return `${hours}h`;
    const days = Math.floor(hours / 24);
    return `${days}d`;
  }

  function shortEndpoint(id: string): string {
    if (id.length <= 16) return id;
    return `${id.slice(0, 8)}…${id.slice(-6)}`;
  }

  async function copyEndpoint(id: string) {
    try {
      await navigator.clipboard.writeText(id);
      copiedId = id;
      if (copyTimeout) clearTimeout(copyTimeout);
      copyTimeout = setTimeout(() => {
        copiedId = null;
      }, 2000);
    } catch {
      // Fallback if clipboard API is unavailable
      copiedId = null;
    }
  }

  async function approve(endpointId: string, name: string) {
    busyEndpoint = endpointId;
    actionError = null;
    actionSuccess = null;
    try {
      await api.approveDevice(endpointId);
      actionSuccess = `Approved ${name} (${shortEndpoint(endpointId)}).`;
      await loadStatus();
    } catch (err) {
      actionError = `Failed to approve device: ${String(err)}`;
    } finally {
      busyEndpoint = null;
    }
  }

  let onlineForwarderCount = $derived(
    forwarders.filter((f) => getLiveness(f.last_seen_unix_ms).online).length,
  );

  let onlineReceiverCount = $derived(
    receivers.filter((r) => getLiveness(r.last_seen_unix_ms).online).length,
  );

  async function loadStatus() {
    try {
      status = await api.getStatus();
      error = null;
    } catch (err) {
      error = String(err);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    void loadStatus();
    poll = setInterval(() => void loadStatus(), 2_000);
    clockTicker = setInterval(() => {
      nowMs = Date.now();
    }, 1_000);
  });

  onDestroy(() => {
    if (poll) clearInterval(poll);
    if (clockTicker) clearInterval(clockTicker);
    if (copyTimeout) clearTimeout(copyTimeout);
  });
</script>

<div class="mx-auto px-4 py-6 space-y-6" style="max-width: 1100px;">
  <!-- Header -->
  <div class="flex flex-wrap items-center justify-between gap-3">
    <div>
      <h1 class="text-2xl font-bold text-text-primary m-0">Server status</h1>
      <p class="text-sm text-text-muted mt-1 mb-0">
        Live connection status for remote timing equipment.
      </p>
    </div>
    <div class="flex items-center gap-2">
      {#if loading}
        <StatusBadge label="Connecting…" state="warn" />
      {:else if error}
        <StatusBadge label="Status stale" state="err" />
      {:else}
        <StatusBadge label="Live" state="ok" />
      {/if}
    </div>
  </div>

  {#if error}
    <AlertBanner variant="err" message={`Could not refresh status: ${error}`} />
  {/if}

  {#if actionSuccess}
    <AlertBanner variant="ok" message={actionSuccess} />
  {/if}

  {#if actionError}
    <AlertBanner variant="err" message={actionError} />
  {/if}

  <!-- Glanceable Summary Card -->
  <Card helpSection="server_status" helpContext="server">
    <div class="grid gap-4 sm:grid-cols-3">
      <StatCard
        label="Forwarders"
        value={status
          ? `${onlineForwarderCount} / ${forwarders.length} online`
          : "—"}
        subtitle={forwarders.length === 0
          ? "No forwarders registered"
          : undefined}
      />
      <StatCard
        label="Receivers"
        value={status
          ? `${onlineReceiverCount} / ${receivers.length} online`
          : "—"}
        subtitle={receivers.length === 0
          ? "No receivers registered"
          : undefined}
      />
      <StatCard
        label="Finishers recorded"
        value={status?.finisher_count ?? "—"}
        subtitle="Live announcer count"
      />
    </div>
  </Card>

  <!-- Split Sections: Forwarders & Receivers -->
  <div class="grid gap-6 lg:grid-cols-2">
    <!-- Forwarders Section -->
    <Card title="Forwarders" helpSection="stream_catalogs" helpContext="server">
      <div class="space-y-3">
        {#if !status}
          <p class="text-sm text-text-muted m-0">Loading forwarders…</p>
        {:else if forwarders.length === 0}
          <div
            class="p-6 text-center rounded-lg border border-dashed border-border bg-surface-0"
          >
            <p class="text-sm text-text-muted m-0">
              No forwarders registered yet.
            </p>
            <p class="text-xs text-text-muted mt-1 mb-0">
              Provision single-board computer forwarders from the <a
                href="/sbc-setup"
                class="text-accent hover:underline">SBC Setup</a
              > page.
            </p>
          </div>
        {:else}
          <div
            class="flex items-center justify-between text-xs text-text-muted font-medium pb-1 border-b border-border/40"
          >
            <span>{forwarders.length} registered</span>
            <span class="text-status-ok font-semibold"
              >{onlineForwarderCount} online</span
            >
          </div>

          <div class="space-y-2.5">
            {#each forwarders as fwd (fwd.endpoint_id)}
              {@const liveness = getLiveness(fwd.last_seen_unix_ms)}
              <div
                class="p-3.5 rounded-lg border border-border bg-surface-1 hover:bg-surface-2/40 transition-colors flex flex-col gap-2.5"
              >
                <!-- Top Row: Name and Badges -->
                <div class="flex items-start justify-between gap-2">
                  <div class="min-w-0 flex-1">
                    <div class="flex items-center gap-2 flex-wrap">
                      <span
                        class="text-base font-semibold text-text-primary truncate"
                      >
                        {fwd.display_name}
                      </span>
                      {#if fwd.approval_state === "pending"}
                        <StatusBadge label="Pending" state="warn" />
                      {/if}
                    </div>

                    <!-- Endpoint ID & Copy -->
                    <div class="flex items-center gap-1.5 mt-1">
                      <span
                        class="text-xs font-mono text-text-muted select-all"
                      >
                        {shortEndpoint(fwd.endpoint_id)}
                      </span>
                      <button
                        type="button"
                        class="text-xs text-text-muted hover:text-text-primary p-0.5 rounded cursor-pointer transition-colors"
                        onclick={() => copyEndpoint(fwd.endpoint_id)}
                        title="Copy full endpoint ID"
                        aria-label="Copy endpoint ID"
                      >
                        {#if copiedId === fwd.endpoint_id}
                          <span class="text-status-ok font-sans text-xs"
                            >Copied!</span
                          >
                        {:else}
                          <svg
                            class="w-3.5 h-3.5 inline opacity-70 hover:opacity-100"
                            fill="none"
                            viewBox="0 0 24 24"
                            stroke="currentColor"
                          >
                            <path
                              stroke-linecap="round"
                              stroke-linejoin="round"
                              stroke-width="2"
                              d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z"
                            />
                          </svg>
                        {/if}
                      </button>
                    </div>
                  </div>

                  <!-- Liveness status badge -->
                  <div class="flex flex-col items-end gap-0.5 shrink-0">
                    <StatusBadge
                      label={liveness.label}
                      state={liveness.state}
                    />
                    <span
                      class="text-[11px] text-text-muted font-medium mt-0.5"
                    >
                      {liveness.detail}
                    </span>
                  </div>
                </div>

                <!-- Readers summary -->
                <div
                  class="pt-2 border-t border-border/50 flex flex-wrap items-center gap-1.5"
                >
                  <span class="text-xs text-text-muted font-medium"
                    >Readers:</span
                  >
                  {#if fwd.streams.length === 0}
                    <span class="text-xs text-text-muted italic"
                      >None reported</span
                    >
                  {:else}
                    {#each fwd.streams as stream (stream.stream_id)}
                      <span
                        class="px-2 py-0.5 rounded text-xs font-mono bg-surface-2 text-text-primary border border-border"
                      >
                        {stream.stream_id}
                      </span>
                    {/each}
                  {/if}
                </div>

                <!-- Inline approve action for pending forwarder -->
                {#if fwd.approval_state === "pending"}
                  <div
                    class="pt-2 border-t border-border/50 flex items-center justify-between gap-2"
                  >
                    <span class="text-xs text-status-warn font-medium"
                      >Awaiting approval</span
                    >
                    <button
                      type="button"
                      class={buttonClass("primary", "xs")}
                      disabled={busyEndpoint === fwd.endpoint_id}
                      onclick={() => approve(fwd.endpoint_id, fwd.display_name)}
                    >
                      {busyEndpoint === fwd.endpoint_id
                        ? "Approving…"
                        : "Approve"}
                    </button>
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        {/if}
      </div>
    </Card>

    <!-- Receivers Section -->
    <Card
      title="Receivers"
      helpSection="registered_devices"
      helpContext="server"
    >
      <div class="space-y-3">
        {#if !status}
          <p class="text-sm text-text-muted m-0">Loading receivers…</p>
        {:else if receivers.length === 0}
          <div
            class="p-6 text-center rounded-lg border border-dashed border-border bg-surface-0"
          >
            <p class="text-sm text-text-muted m-0">
              No receivers registered yet.
            </p>
            <p class="text-xs text-text-muted mt-1 mb-0">
              Generate enrollment tokens on the <a
                href="/admin"
                class="text-accent hover:underline">Admin</a
              > page to connect receivers.
            </p>
          </div>
        {:else}
          <div
            class="flex items-center justify-between text-xs text-text-muted font-medium pb-1 border-b border-border/40"
          >
            <span>{receivers.length} registered</span>
            <span class="text-status-ok font-semibold"
              >{onlineReceiverCount} online</span
            >
          </div>

          <div class="space-y-2.5">
            {#each receivers as rx (rx.endpoint_id)}
              {@const liveness = getLiveness(rx.last_seen_unix_ms)}
              <div
                class="p-3.5 rounded-lg border border-border bg-surface-1 hover:bg-surface-2/40 transition-colors flex flex-col gap-2.5"
              >
                <!-- Top Row: Name and Badges -->
                <div class="flex items-start justify-between gap-2">
                  <div class="min-w-0 flex-1">
                    <div class="flex items-center gap-2 flex-wrap">
                      <span
                        class="text-base font-semibold text-text-primary truncate"
                      >
                        {rx.display_name}
                      </span>
                      <StatusBadge
                        label={rx.approval_state}
                        state={approvalBadgeState(rx.approval_state)}
                      />
                    </div>

                    <!-- Endpoint ID & Copy -->
                    <div class="flex items-center gap-1.5 mt-1">
                      <span
                        class="text-xs font-mono text-text-muted select-all"
                      >
                        {shortEndpoint(rx.endpoint_id)}
                      </span>
                      <button
                        type="button"
                        class="text-xs text-text-muted hover:text-text-primary p-0.5 rounded cursor-pointer transition-colors"
                        onclick={() => copyEndpoint(rx.endpoint_id)}
                        title="Copy full endpoint ID"
                        aria-label="Copy endpoint ID"
                      >
                        {#if copiedId === rx.endpoint_id}
                          <span class="text-status-ok font-sans text-xs"
                            >Copied!</span
                          >
                        {:else}
                          <svg
                            class="w-3.5 h-3.5 inline opacity-70 hover:opacity-100"
                            fill="none"
                            viewBox="0 0 24 24"
                            stroke="currentColor"
                          >
                            <path
                              stroke-linecap="round"
                              stroke-linejoin="round"
                              stroke-width="2"
                              d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z"
                            />
                          </svg>
                        {/if}
                      </button>
                    </div>
                  </div>

                  <!-- Liveness status badge -->
                  <div class="flex flex-col items-end gap-0.5 shrink-0">
                    <StatusBadge
                      label={liveness.label}
                      state={liveness.state}
                    />
                    <span
                      class="text-[11px] text-text-muted font-medium mt-0.5"
                    >
                      {liveness.detail}
                    </span>
                  </div>
                </div>

                <!-- Inline approve action for pending receiver -->
                {#if rx.approval_state === "pending"}
                  <div
                    class="pt-2 border-t border-border/50 flex items-center justify-between gap-2"
                  >
                    <span class="text-xs text-status-warn font-medium"
                      >Awaiting approval</span
                    >
                    <button
                      type="button"
                      class={buttonClass("primary", "xs")}
                      disabled={busyEndpoint === rx.endpoint_id}
                      onclick={() => approve(rx.endpoint_id, rx.display_name)}
                    >
                      {busyEndpoint === rx.endpoint_id
                        ? "Approving…"
                        : "Approve"}
                    </button>
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        {/if}
      </div>
    </Card>
  </div>
</div>
