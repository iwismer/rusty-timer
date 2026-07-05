<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import {
    AlertBanner,
    Card,
    StatCard,
    StatusBadge,
    approvalBadgeState,
    tableClass,
    tableHeadRowClass,
    tableRowClass,
    tableCellClass,
    tableHeaderCellClass,
  } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";
  import type { DeviceKind, DeviceRecord, StatusResponse } from "$lib/api";

  let status = $state<StatusResponse | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(true);
  let poll: ReturnType<typeof setInterval> | undefined;

  let streamForwarderNames = $derived.by(() => {
    const names: Record<string, string> = {};
    for (const forwarder of status?.forwarders ?? []) {
      names[forwarder.endpoint_id] =
        forwarder.display_name ?? forwarder.endpoint_id;
    }
    return names;
  });
  let streamDevices = $derived.by(() => {
    const devices: Record<string, DeviceRecord> = {};
    if (!status) return devices;
    for (const device of status.devices) {
      devices[device.endpoint_id] = device;
    }
    return devices;
  });
  let forwardersWithoutStreams = $derived.by(() => {
    if (!status) return [];
    const streamEndpoints = new Set(
      status.forwarder_streams.map((stream) => stream.endpoint_id),
    );
    return status.forwarders.filter(
      (forwarder) => !streamEndpoints.has(forwarder.endpoint_id),
    );
  });

  function kindLabel(kind: DeviceKind) {
    return kind === "forwarder" ? "Forwarder" : "Receiver";
  }

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
  });

  onDestroy(() => {
    if (poll) clearInterval(poll);
  });
</script>

<div class="mx-auto px-4 py-6 space-y-6" style="max-width: 1100px;">
  <div class="flex flex-wrap items-center justify-between gap-3">
    <div>
      <h1 class="text-2xl font-bold text-text-primary m-0">Server status</h1>
      <p class="text-sm text-text-muted mt-1 mb-0">
        Registered devices, forwarder stream catalogs, and announcer health.
      </p>
    </div>
    {#if loading}
      <StatusBadge label="Loading" state="warn" />
    {:else if error}
      <StatusBadge label="Status stale" state="err" />
    {:else}
      <StatusBadge label="Live" state="ok" />
    {/if}
  </div>

  {#if error}
    <AlertBanner variant="err" message={`Could not refresh status: ${error}`} />
  {/if}

  <div class="grid gap-4 sm:grid-cols-3">
    <Card>
      <StatCard
        label="Announcer generation"
        value={status?.announcer_source_generation ?? "—"}
      />
    </Card>
    <Card>
      <StatCard label="Finishers" value={status?.finisher_count ?? "—"} />
    </Card>
    <Card>
      <StatCard
        label="Registered devices"
        value={status?.devices.length ?? "—"}
      />
    </Card>
  </div>

  <Card title="Connected forwarders and stream catalogs">
    {#if !status}
      <p class="text-sm text-text-muted m-0">Loading forwarders…</p>
    {:else if status.forwarders.length === 0 && status.forwarder_streams.length === 0}
      <p class="text-sm text-text-muted m-0">
        No forwarders or streams registered.
      </p>
    {:else}
      <div class="overflow-x-auto">
        <table class={tableClass}>
          <thead>
            <tr class={tableHeadRowClass}>
              <th class={tableHeaderCellClass()}>Forwarder</th>
              <th class={tableHeaderCellClass()}>Stream</th>
              <th class={tableHeaderCellClass()}>Epoch</th>
              <th class={tableHeaderCellClass()}>Next seq</th>
              <th class={tableHeaderCellClass()}>Approval</th>
            </tr>
          </thead>
          <tbody>
            {#each status.forwarder_streams as stream (stream.stream_id)}
              <tr class={tableRowClass}>
                <td class={tableCellClass(false, "text-text-primary")}>
                  {streamForwarderNames[stream.endpoint_id] ??
                    stream.endpoint_id}
                  <span class="block text-xs text-text-muted font-mono">
                    {stream.endpoint_id}
                  </span>
                </td>
                <td class={tableCellClass(false, "text-text-primary font-mono")}
                  >{stream.stream_id}</td
                >
                <td class={tableCellClass(false, "text-text-primary font-mono")}
                  >{stream.epoch}</td
                >
                <td class={tableCellClass(false, "text-text-primary font-mono")}
                  >{stream.next_seq}</td
                >
                <td class={tableCellClass()}>
                  {#if streamDevices[stream.endpoint_id]}
                    <StatusBadge
                      label={streamDevices[stream.endpoint_id].approval_state}
                      state={approvalBadgeState(
                        streamDevices[stream.endpoint_id].approval_state,
                      )}
                    />
                  {:else}
                    <StatusBadge label="unknown" state="warn" />
                  {/if}
                </td>
              </tr>
            {/each}
            {#each forwardersWithoutStreams as forwarder (forwarder.endpoint_id)}
              <tr class={tableRowClass}>
                <td class={tableCellClass(false, "text-text-primary")}>
                  {forwarder.display_name ?? forwarder.endpoint_id}
                  <span class="block text-xs text-text-muted font-mono">
                    {forwarder.endpoint_id}
                  </span>
                </td>
                <td class={tableCellClass(false, "text-text-muted")} colspan="3"
                  >No streams reported</td
                >
                <td class={tableCellClass()}>
                  <StatusBadge
                    label={forwarder.approval_state}
                    state={approvalBadgeState(forwarder.approval_state)}
                  />
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </Card>

  <Card title="Registered devices">
    {#if !status}
      <p class="text-sm text-text-muted m-0">Loading devices…</p>
    {:else if status.devices.length === 0}
      <p class="text-sm text-text-muted m-0">No devices registered.</p>
    {:else}
      <div class="overflow-x-auto">
        <table class={tableClass}>
          <thead>
            <tr class={tableHeadRowClass}>
              <th class={tableHeaderCellClass()}>Endpoint</th>
              <th class={tableHeaderCellClass()}>Kind</th>
              <th class={tableHeaderCellClass()}>Approval</th>
            </tr>
          </thead>
          <tbody>
            {#each status.devices as device (device.endpoint_id)}
              <tr class={tableRowClass}>
                <td class={tableCellClass(false, "text-text-primary font-mono")}
                  >{device.endpoint_id}</td
                >
                <td class={tableCellClass(false, "text-text-primary")}
                  >{kindLabel(device.device_kind)}</td
                >
                <td class={tableCellClass()}>
                  <StatusBadge
                    label={device.approval_state}
                    state={approvalBadgeState(device.approval_state)}
                  />
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </Card>
</div>
