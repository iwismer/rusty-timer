<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { AlertBanner, Card, StatusBadge } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";
  import type { DeviceRecord, StatusResponse } from "$lib/api";

  let status = $state<StatusResponse | null>(null);
  let error = $state<string | null>(null);
  let success = $state<string | null>(null);
  let loading = $state(true);
  let busyEndpoint = $state<string | null>(null);
  let poll: ReturnType<typeof setInterval> | undefined;

  let pendingDevices = $derived(
    status?.devices.filter((device) => device.approval_state === "pending") ??
      [],
  );

  let activeDevices = $derived(
    status?.devices.filter((device) => device.approval_state === "active") ??
      [],
  );

  function displayKind(device: DeviceRecord) {
    return device.device_kind === "forwarder" ? "Forwarder" : "Receiver";
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

  async function approve(device: DeviceRecord) {
    busyEndpoint = device.endpoint_id;
    error = null;
    success = null;
    try {
      const approved = await api.approveDevice(device.endpoint_id);
      success = `Approved ${approved.endpoint_id}.`;
      await loadStatus();
    } catch (err) {
      error = String(err);
    } finally {
      busyEndpoint = null;
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
      <h1 class="text-2xl font-bold text-text-primary m-0">Device approval</h1>
      <p class="text-sm text-text-muted mt-1 mb-0">
        Approve pending forwarders and receivers. Forwarders provide their own
        display names; receivers are shown by endpoint ID.
      </p>
    </div>
    {#if loading}
      <StatusBadge label="Loading" state="warn" />
    {:else if error}
      <StatusBadge label="Action needed" state="err" />
    {:else}
      <StatusBadge
        label={`${pendingDevices.length} pending`}
        state={pendingDevices.length ? "warn" : "ok"}
      />
    {/if}
  </div>

  {#if error}
    <AlertBanner variant="err" message={error} />
  {/if}
  {#if success}
    <AlertBanner
      variant="ok"
      message={success}
      onDismiss={() => (success = null)}
    />
  {/if}

  <Card title="Pending devices">
    {#if !status}
      <p class="text-sm text-text-muted m-0">Loading devices…</p>
    {:else if pendingDevices.length === 0}
      <p class="text-sm text-text-muted m-0">
        No devices are pending approval.
      </p>
    {:else}
      <div class="space-y-4">
        {#each pendingDevices as device (device.endpoint_id)}
          <form
            class="grid gap-3 rounded-md border border-border bg-surface-2 p-4 md:grid-cols-[1fr_auto] md:items-center"
            onsubmit={(event) => {
              event.preventDefault();
              void approve(device);
            }}
          >
            <div>
              <p class="text-sm font-semibold text-text-primary m-0">
                {displayKind(device)}
              </p>
              <p class="text-xs text-text-muted font-mono mt-1 mb-0">
                {device.endpoint_id}
              </p>
            </div>
            <button
              type="submit"
              class="rounded-md border-none bg-accent px-4 py-2 text-sm font-medium text-white hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-50"
              disabled={busyEndpoint === device.endpoint_id}
            >
              {busyEndpoint === device.endpoint_id ? "Approving…" : "Approve"}
            </button>
          </form>
        {/each}
      </div>
    {/if}
  </Card>

  <Card title="Approved devices">
    {#if !status}
      <p class="text-sm text-text-muted m-0">Loading devices…</p>
    {:else if activeDevices.length === 0}
      <p class="text-sm text-text-muted m-0">No approved devices yet.</p>
    {:else}
      <div class="space-y-3">
        {#each activeDevices as device (device.endpoint_id)}
          <div class="rounded-md border border-border bg-surface-2 p-4">
            <p class="text-sm font-semibold text-text-primary m-0">
              {displayKind(device)}
            </p>
            <p class="text-xs text-text-muted font-mono mt-1 mb-0">
              {device.endpoint_id}
            </p>
          </div>
        {/each}
      </div>
    {/if}
  </Card>
</div>
