<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import {
    store,
    loadForwarders,
    selectForwarder,
    setForwarderRace,
  } from "$lib/store.svelte";
  import {
    ForwarderConfig,
    BatteryIndicator,
    LowBatteryBanner,
  } from "@rusty-timer/shared-ui";
  import type { ConfigApi } from "@rusty-timer/shared-ui";
  import { createForwarderConfigApi } from "$lib/forwarder-config-api";
  import type { ForwarderEntry } from "$lib/api";

  let now = $state(Date.now());
  let tickInterval: ReturnType<typeof setInterval>;

  onMount(() => {
    void loadForwarders();
    tickInterval = setInterval(() => {
      now = Date.now();
    }, 1000);
  });

  onDestroy(() => {
    clearInterval(tickInterval);
  });

  function selectedForwarder(): ForwarderEntry | undefined {
    if (!store.selectedForwarderId || !store.forwarders) return undefined;
    return store.forwarders.find(
      (f) => f.forwarder_id === store.selectedForwarderId,
    );
  }

  function readerDotClass(connected: boolean): string {
    return connected ? "bg-status-ok" : "bg-status-err";
  }

  function forwarderDotClass(fwd: ForwarderEntry): string {
    if (!fwd.online) return "bg-status-err";
    if (fwd.readers.some((r) => !r.connected)) return "bg-status-warn";
    return "bg-status-ok";
  }

  function formatLastRead(lastReadAt: string | null, _now: number): string {
    if (!lastReadAt) return "\u2014";
    const diff = _now - new Date(lastReadAt).getTime();
    const secs = Math.max(0, Math.floor(diff / 1000));
    if (secs < 60) return `${secs}s ago`;
    const mins = Math.floor(secs / 60);
    if (mins < 60) return `${mins}m ago`;
    const hours = Math.floor(mins / 60);
    return `${hours}h ago`;
  }

  function lastReadColor(lastReadAt: string | null, _now: number): string {
    if (!lastReadAt) return "text-text-muted";
    const diff = _now - new Date(lastReadAt).getTime();
    if (diff < 10_000) return "text-status-ok";
    if (diff < 60_000) return "text-text-primary";
    return "text-text-muted";
  }

  let configApi: ConfigApi | null = $derived(
    store.selectedForwarderId
      ? createForwarderConfigApi(store.selectedForwarderId)
      : null,
  );

  let lowBatteryForwarders = $derived(
    (store.forwarders ?? [])
      .filter((f) => {
        const ups = store.upsState.get(f.forwarder_id);
        return (
          ups?.status &&
          ups.status.battery_percent <= 15 &&
          !ups.status.power_plugged
        );
      })
      .map((f) => ({
        name: f.display_name ?? f.forwarder_id,
        percent: store.upsState.get(f.forwarder_id)!.status!.battery_percent,
      })),
  );
</script>

<div class="h-full flex flex-col">
  <LowBatteryBanner forwarders={lowBatteryForwarders} />

  {#if store.selectedForwarderId && selectedForwarder()}
    {@const fwd = selectedForwarder()!}
    <!-- Detail view -->
    <div class="flex-1 overflow-y-auto">
      <div class="max-w-[900px] mx-auto px-6 py-6">
        <!-- Back + header -->
        <div class="mb-4">
          <button
            class="text-xs text-accent bg-transparent border-none cursor-pointer hover:underline"
            onclick={() => selectForwarder(null)}
          >
            &larr; Back to forwarders
          </button>
        </div>

        <div class="flex items-center gap-3 mb-6">
          <span class="w-3 h-3 rounded-full shrink-0 {forwarderDotClass(fwd)}"
          ></span>
          <h1 class="text-xl font-bold text-text-primary m-0">
            {fwd.display_name ?? fwd.forwarder_id}
          </h1>
          {#if fwd.display_name}
            <span class="text-xs text-text-muted font-mono">
              {fwd.forwarder_id}
            </span>
          {/if}
        </div>

        <!-- Stats cards -->
        <div class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-3 mb-6">
          <div class="bg-surface-0 border border-border rounded-lg px-4 py-3">
            <div class="text-[11px] text-text-muted uppercase tracking-wide">
              Unique Chips
            </div>
            <div class="text-2xl font-bold text-text-primary font-mono mt-1">
              {fwd.unique_chips.toLocaleString()}
            </div>
          </div>
          <div class="bg-surface-0 border border-border rounded-lg px-4 py-3">
            <div class="text-[11px] text-text-muted uppercase tracking-wide">
              Total Reads
            </div>
            <div class="text-2xl font-bold text-text-primary font-mono mt-1">
              {fwd.total_reads.toLocaleString()}
            </div>
          </div>
          <div class="bg-surface-0 border border-border rounded-lg px-4 py-3">
            <div class="text-[11px] text-text-muted uppercase tracking-wide">
              Readers
            </div>
            <div
              class="text-2xl font-bold font-mono mt-1 {fwd.readers.every(
                (r) => r.connected,
              ) && fwd.online
                ? 'text-status-ok'
                : 'text-text-primary'}"
            >
              {fwd.readers.filter((r) => r.connected).length} / {fwd.readers
                .length}
            </div>
            <div class="text-[11px] text-text-muted">
              {fwd.readers.every((r) => r.connected) && fwd.online
                ? "all connected"
                : "some disconnected"}
            </div>
          </div>
          <div class="bg-surface-0 border border-border rounded-lg px-4 py-3">
            <div class="text-[11px] text-text-muted uppercase tracking-wide">
              Last Read
            </div>
            <div
              class="text-2xl font-bold font-mono mt-1 {lastReadColor(
                fwd.last_read_at,
                now,
              )}"
            >
              {formatLastRead(fwd.last_read_at, now)}
            </div>
          </div>
          {#if store.upsState.get(fwd.forwarder_id)}
            {@const upsEntry = store.upsState.get(fwd.forwarder_id)!}
            <div class="bg-surface-0 border border-border rounded-lg px-4 py-3">
              <div class="text-[11px] text-text-muted uppercase tracking-wide">
                Battery
              </div>
              <div class="flex items-center gap-2 mt-1">
                <BatteryIndicator
                  percent={upsEntry.status?.battery_percent ?? null}
                  charging={upsEntry.status?.charging ?? false}
                  available={upsEntry.available}
                  configured
                />
              </div>
              {#if upsEntry.status}
                <div class="text-xs text-text-muted mt-1">
                  {(upsEntry.status.battery_voltage_mv / 1000).toFixed(2)}V / {(
                    upsEntry.status.temperature_cdeg / 100
                  ).toFixed(1)}&deg;C
                </div>
                <div class="text-xs mt-1">
                  {#if upsEntry.status.charging}
                    Charging
                  {:else if upsEntry.status.power_plugged}
                    Plugged In
                  {:else}
                    On Battery
                  {/if}
                </div>
              {:else if !upsEntry.available}
                <div class="text-xs text-text-muted mt-1">UPS unavailable</div>
              {/if}
            </div>
          {/if}
        </div>

        <!-- Race assignment -->
        <div
          class="bg-surface-0 border border-border rounded-lg mb-6 overflow-hidden"
        >
          <div
            class="px-4 py-3 border-b border-border font-semibold text-text-primary text-sm"
          >
            Race Assignment
          </div>
          <div class="px-4 py-3 flex flex-col gap-2">
            <div class="flex items-center gap-3">
              {#if store.forwarderRaceLoading}
                <span class="text-sm text-text-muted">Loading...</span>
              {:else}
                <select
                  class="flex-1 bg-surface-1 border border-border rounded px-3 py-1.5 text-sm text-text-primary"
                  value={store.forwarderRaceId ?? ""}
                  disabled={store.forwarderRaceSaving}
                  onchange={(e) => {
                    const val = e.currentTarget.value;
                    void setForwarderRace(fwd.forwarder_id, val || null);
                  }}
                >
                  <option value="">No race assigned</option>
                  {#each store.races as race (race.race_id)}
                    <option value={race.race_id}>{race.name}</option>
                  {/each}
                </select>
                {#if store.forwarderRaceSaving}
                  <span class="text-xs text-text-muted">Saving...</span>
                {/if}
              {/if}
            </div>
            {#if store.forwarderRaceError}
              <p class="text-xs text-status-err m-0">
                {store.forwarderRaceError}
              </p>
            {/if}
          </div>
        </div>

        <!-- Readers table -->
        <div
          class="bg-surface-0 border border-border rounded-lg mb-6 overflow-hidden"
        >
          <div
            class="px-4 py-3 border-b border-border font-semibold text-text-primary text-sm"
          >
            Readers
          </div>
          <table class="w-full border-collapse text-sm">
            <thead>
              <tr
                class="border-b border-border text-left text-text-muted text-[11px] uppercase tracking-wide"
              >
                <th class="px-4 py-2">Status</th>
                <th class="px-4 py-2">IP Address</th>
              </tr>
            </thead>
            <tbody>
              {#each fwd.readers as reader}
                <tr class="border-b border-border/50">
                  <td class="px-4 py-2">
                    <span
                      class="w-2 h-2 rounded-full inline-block {readerDotClass(
                        reader.connected,
                      )}"
                    ></span>
                  </td>
                  <td class="px-4 py-2 font-mono">{reader.reader_ip}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>

        <!-- Forwarder config -->
        {#if configApi}
          <ForwarderConfig
            {configApi}
            displayName={fwd.display_name ?? fwd.forwarder_id}
            isOnline={fwd.online}
          />
        {/if}
      </div>
    </div>
  {:else}
    <!-- List view -->
    {#if !store.forwarders && store.forwardersError}
      <div
        class="flex-1 flex items-center justify-center text-text-muted text-sm"
      >
        Unable to load forwarders: {store.forwardersError}
      </div>
    {:else if !store.forwarders}
      <div
        class="flex-1 flex items-center justify-center text-text-muted text-sm"
      >
        Loading forwarders...
      </div>
    {:else if store.forwarders.length === 0}
      <div
        class="flex-1 flex items-center justify-center text-text-muted text-sm"
      >
        No forwarders found. Connect a forwarder to the server to see it here.
      </div>
    {:else}
      <div class="flex-1 overflow-y-auto">
        <table class="w-full border-collapse text-sm">
          <thead>
            <tr
              class="sticky top-0 z-10 bg-surface-0 border-b border-border text-left text-text-muted text-[11px] uppercase tracking-wide"
            >
              <th class="px-3 py-2">Forwarder</th>
              <th class="px-3 py-2">Battery</th>
              <th class="px-3 py-2">Readers</th>
              <th class="px-3 py-2 text-right">Unique Chips</th>
              <th class="px-3 py-2 text-right">Total Reads</th>
              <th class="px-3 py-2 text-right">Last Read</th>
            </tr>
          </thead>
          <tbody>
            {#each store.forwarders as fwd (fwd.forwarder_id)}
              <tr
                class="border-b border-border/50 hover:bg-surface-1/50 cursor-pointer {!fwd.online
                  ? 'opacity-60'
                  : ''}"
                role="button"
                tabindex="0"
                onclick={() => selectForwarder(fwd.forwarder_id)}
                onkeydown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    selectForwarder(fwd.forwarder_id);
                  }
                }}
              >
                <td class="px-3 py-2.5">
                  <div class="flex items-center gap-2">
                    <span
                      class="w-2.5 h-2.5 rounded-full shrink-0 {forwarderDotClass(
                        fwd,
                      )}"
                    ></span>
                    <div>
                      <div class="font-semibold text-text-primary">
                        {fwd.display_name ?? fwd.forwarder_id}
                      </div>
                      {#if fwd.display_name}
                        <div class="text-[11px] text-text-muted font-mono">
                          {fwd.forwarder_id}
                        </div>
                      {/if}
                    </div>
                  </div>
                </td>
                <td>
                  <BatteryIndicator
                    percent={store.upsState.get(fwd.forwarder_id)?.status
                      ?.battery_percent ?? null}
                    charging={store.upsState.get(fwd.forwarder_id)?.status
                      ?.charging ?? false}
                    available={store.upsState.get(fwd.forwarder_id)
                      ?.available ?? true}
                    configured={store.upsState.get(fwd.forwarder_id) !==
                      undefined}
                    compact
                  />
                </td>
                <td class="px-3 py-2.5">
                  <div class="flex flex-col gap-0.5">
                    {#each fwd.readers as reader}
                      <div class="flex items-center gap-1.5">
                        <span
                          class="w-1.5 h-1.5 rounded-full {readerDotClass(
                            reader.connected,
                          )}"
                        ></span>
                        <span class="font-mono text-xs">{reader.reader_ip}</span
                        >
                      </div>
                    {/each}
                  </div>
                </td>
                <td class="px-3 py-2.5 text-right font-mono text-text-primary">
                  {fwd.unique_chips > 0
                    ? fwd.unique_chips.toLocaleString()
                    : "\u2014"}
                </td>
                <td class="px-3 py-2.5 text-right font-mono text-text-primary">
                  {fwd.total_reads > 0
                    ? fwd.total_reads.toLocaleString()
                    : "\u2014"}
                </td>
                <td
                  class="px-3 py-2.5 text-right font-mono {lastReadColor(
                    fwd.last_read_at,
                    now,
                  )}"
                >
                  {formatLastRead(fwd.last_read_at, now)}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/if}
</div>
