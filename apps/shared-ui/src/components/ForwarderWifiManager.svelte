<script lang="ts">
  import { onMount } from "svelte";
  import Card from "./Card.svelte";
  import StatusBadge from "./StatusBadge.svelte";
  import ConfirmDialog from "./ConfirmDialog.svelte";
  import {
    extractWifiErrorMessage,
    isLikelyNetworkDropError,
    isNetworkSecured,
    signalBars,
    wifiStateBadge,
    type WifiApi,
    type WifiNetwork,
    type WifiStatus,
  } from "../lib/wifi-types";

  let {
    wifiApi,
    isRemoteP2p = false,
  }: {
    wifiApi: WifiApi;
    isRemoteP2p?: boolean;
  } = $props();

  let loading = $state(true);
  let scanning = $state(false);
  let actionBusy = $state(false);
  let status = $state<WifiStatus | null>(null);
  let networks = $state<WifiNetwork[]>([]);
  let errorBanner = $state<string | null>(null);
  let infoBanner = $state<string | null>(null);

  // Inline password prompt state for a visible network
  let selectedSsid = $state<string | null>(null);
  let selectedPassword = $state("");
  let showPassword = $state(false);

  // Hidden network collapsible form state
  let showHiddenForm = $state(false);
  let hiddenSsid = $state("");
  let hiddenSecurity = $state<"wpa" | "open">("wpa");
  let hiddenPassword = $state("");
  let showHiddenPassword = $state(false);

  // Disconnect and Forget confirmation dialogs
  let confirmDisconnectOpen = $state(false);
  let confirmForgetSsid = $state<string | null>(null);

  const inputClass =
    "px-2.5 py-1.5 text-sm rounded-md border border-border bg-surface-0 text-text-primary focus:outline-none focus:border-accent";

  async function loadInitial() {
    loading = true;
    errorBanner = null;
    try {
      const s = await wifiApi.getStatus();
      status = s;
      if (s.available && s.enabled) {
        const scanResult = await wifiApi.scan(false);
        status = scanResult.status;
        networks = scanResult.networks;
      }
    } catch (e) {
      errorBanner = extractWifiErrorMessage(e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    void loadInitial();
  });

  async function handleRefreshStatus() {
    errorBanner = null;
    infoBanner = null;
    actionBusy = true;
    try {
      const s = await wifiApi.getStatus();
      status = s;
      if (s.available && s.enabled) {
        const scanResult = await wifiApi.scan(false);
        status = scanResult.status;
        networks = scanResult.networks;
      }
    } catch (e) {
      errorBanner = extractWifiErrorMessage(e);
    } finally {
      actionBusy = false;
    }
  }

  async function handleScan(rescan = true) {
    errorBanner = null;
    infoBanner = null;
    scanning = true;
    try {
      const scanResult = await wifiApi.scan(rescan);
      status = scanResult.status;
      networks = scanResult.networks;
    } catch (e) {
      errorBanner = extractWifiErrorMessage(e);
    } finally {
      scanning = false;
    }
  }

  function startConnectRow(network: WifiNetwork, forcePasswordPrompt = false) {
    errorBanner = null;
    infoBanner = null;
    const needsPassword =
      isNetworkSecured(network.security) &&
      (!network.saved || forcePasswordPrompt);
    if (needsPassword) {
      selectedSsid = network.ssid;
      selectedPassword = "";
      showPassword = false;
    } else {
      selectedSsid = null;
      void executeConnect({ ssid: network.ssid });
    }
  }

  function cancelConnectRow() {
    selectedSsid = null;
    selectedPassword = "";
    showPassword = false;
  }

  async function executeConnect(params: {
    ssid: string;
    password?: string | null;
    hidden?: boolean;
  }) {
    errorBanner = null;
    infoBanner = null;
    actionBusy = true;
    try {
      const result = await wifiApi.connect(params);
      if (result.status) {
        status = result.status;
      }
      if (!result.ok) {
        errorBanner = result.message || `Failed to connect to "${params.ssid}"`;
      } else {
        infoBanner = result.message || `Connected to "${params.ssid}"`;
        selectedSsid = null;
        selectedPassword = "";
        if (params.hidden) {
          hiddenSsid = "";
          hiddenPassword = "";
          showHiddenForm = false;
        }
        try {
          const refreshed = await wifiApi.scan(false);
          status = refreshed.status;
          networks = refreshed.networks;
        } catch {
          // Ignore follow-up scan failure if link briefly blipped
        }
      }
    } catch (e) {
      if (isLikelyNetworkDropError(e)) {
        infoBanner = isRemoteP2p
          ? `Switching Wi-Fi to "${params.ssid}" interrupted the P2P connection. The forwarder should reconnect once the new network has internet access.`
          : `Connection to the forwarder dropped while switching to "${params.ssid}". If you were connected over Wi-Fi, reconnect to the new network or check its new IP address.`;
        setTimeout(() => {
          void wifiApi
            .getStatus()
            .then((s) => {
              status = s;
            })
            .catch(() => {});
        }, 4000);
      } else {
        errorBanner = extractWifiErrorMessage(e);
      }
    } finally {
      actionBusy = false;
    }
  }

  async function handleHiddenSubmit(e: Event) {
    e.preventDefault();
    const ssid = hiddenSsid.trim();
    if (!ssid) {
      errorBanner = "SSID is required for hidden network";
      return;
    }
    const password =
      hiddenSecurity === "wpa" ? hiddenPassword || null : null;
    if (hiddenSecurity === "wpa" && !password) {
      errorBanner = "Password is required for a secured hidden network";
      return;
    }
    await executeConnect({
      ssid,
      password,
      hidden: true,
    });
  }

  async function handleConfirmDisconnect() {
    confirmDisconnectOpen = false;
    errorBanner = null;
    infoBanner = null;
    actionBusy = true;
    try {
      const result = await wifiApi.disconnect();
      if (result.status) {
        status = result.status;
      }
      if (!result.ok) {
        errorBanner = result.message || "Failed to disconnect Wi-Fi";
      } else {
        infoBanner = result.message || "Wi-Fi disconnected";
        try {
          const refreshed = await wifiApi.scan(false);
          status = refreshed.status;
          networks = refreshed.networks;
        } catch {
          // Ignore follow-up scan error
        }
      }
    } catch (e) {
      if (isLikelyNetworkDropError(e)) {
        infoBanner =
          "Connection to the forwarder dropped after disconnecting Wi-Fi.";
      } else {
        errorBanner = extractWifiErrorMessage(e);
      }
    } finally {
      actionBusy = false;
    }
  }

  async function handleConfirmForget() {
    const ssid = confirmForgetSsid;
    if (!ssid) return;
    confirmForgetSsid = null;
    errorBanner = null;
    infoBanner = null;
    actionBusy = true;
    try {
      const result = await wifiApi.forget(ssid);
      if (result.status) {
        status = result.status;
      }
      if (!result.ok) {
        errorBanner = result.message || `Failed to forget "${ssid}"`;
      } else {
        infoBanner = result.message || `Removed saved network "${ssid}"`;
        networks = networks.map((n) =>
          n.ssid === ssid ? { ...n, saved: false } : n,
        );
      }
    } catch (e) {
      if (isLikelyNetworkDropError(e)) {
        infoBanner = `Connection to the forwarder dropped after forgetting "${ssid}".`;
      } else {
        errorBanner = extractWifiErrorMessage(e);
      }
    } finally {
      actionBusy = false;
    }
  }

  let badge = $derived(
    status ? wifiStateBadge(status) : { label: "loading", state: "warn" as const },
  );
</script>

<div class="space-y-4">
  {#if errorBanner}
    <div
      data-testid="wifi-error-banner"
      class="px-4 py-3 rounded-lg bg-status-err-bg border border-status-err-border text-status-err text-sm flex items-center justify-between gap-3"
    >
      <span>{errorBanner}</span>
      <button
        type="button"
        onclick={() => (errorBanner = null)}
        class="text-xs underline cursor-pointer bg-transparent border-none text-status-err"
      >
        Dismiss
      </button>
    </div>
  {/if}

  {#if infoBanner}
    <div
      data-testid="wifi-info-banner"
      class="px-4 py-3 rounded-lg bg-status-ok-bg border border-status-ok-border text-status-ok text-sm flex items-center justify-between gap-3"
    >
      <span>{infoBanner}</span>
      <button
        type="button"
        onclick={() => (infoBanner = null)}
        class="text-xs underline cursor-pointer bg-transparent border-none text-status-ok"
      >
        Dismiss
      </button>
    </div>
  {/if}

  {#if loading}
    <p class="text-sm text-text-muted">Loading Wi-Fi status&hellip;</p>
  {:else if status && !status.available}
    <Card title="Wi-Fi Unavailable">
      <p class="text-sm text-text-secondary m-0">
        No NetworkManager-managed Wi-Fi interface was detected on this forwarder
        (or Wi-Fi control is not available on this host).
      </p>
    </Card>
  {:else if status}
    <!-- Current Connection Card -->
    <Card
      helpSection="wifi"
      helpContext="forwarder"
    >
      {#snippet header()}
        <h2 class="text-sm font-semibold text-text-primary m-0">
          Current Wi-Fi Connection
        </h2>
        <div class="ml-auto">
          <StatusBadge label={badge.label} state={badge.state} />
        </div>
      {/snippet}

      <dl
        class="grid grid-cols-[auto_1fr] sm:grid-cols-[auto_1fr_auto_1fr] gap-x-4 gap-y-1.5 text-sm m-0"
      >
        <dt class="text-text-muted">Interface</dt>
        <dd class="font-mono text-xs self-center">
          {status.interface ?? "\u2014"}
        </dd>

        <dt class="text-text-muted">Network (SSID)</dt>
        <dd class="font-medium text-text-primary">
          {status.ssid ?? "Not connected"}
        </dd>

        <dt class="text-text-muted">IPv4 Address</dt>
        <dd class="font-mono text-xs self-center">
          {status.ip_address ?? "\u2014"}
        </dd>

        <dt class="text-text-muted">Gateway</dt>
        <dd class="font-mono text-xs self-center">
          {status.gateway ?? "\u2014"}
        </dd>

        <dt class="text-text-muted">Band / Security</dt>
        <dd class="text-xs self-center">
          {[status.band, status.security].filter(Boolean).join(" \u00b7 ") ||
            "\u2014"}
        </dd>

        <dt class="text-text-muted">Signal</dt>
        <dd class="flex items-center gap-2 self-center">
          {#if status.signal != null}
            {@const bars = signalBars(status.signal)}
            <span
              class="inline-flex items-end gap-0.5 h-3.5"
              aria-label="Signal {status.signal}%"
            >
              {#each [1, 2, 3, 4] as bar (bar)}
                <span
                  class="w-1 rounded-xs {bar <= bars
                    ? 'bg-status-ok'
                    : 'bg-surface-3'}"
                  style="height: {bar * 25}%"
                ></span>
              {/each}
            </span>
            <span class="font-mono text-xs">{status.signal}%</span>
          {:else}
            <span class="font-mono text-xs">{"\u2014"}</span>
          {/if}
        </dd>
      </dl>

      <div class="mt-4 pt-3 border-t border-border flex flex-wrap gap-2">
        <button
          type="button"
          data-testid="wifi-refresh-btn"
          onclick={handleRefreshStatus}
          disabled={actionBusy || scanning}
          class="px-3 py-1.5 text-xs font-medium rounded-md border border-border bg-surface-2 text-text-primary cursor-pointer hover:bg-surface-3 disabled:opacity-40 disabled:cursor-not-allowed"
        >
          Refresh Status
        </button>

        {#if status.state === "connected"}
          <button
            type="button"
            data-testid="wifi-disconnect-btn"
            onclick={() => (confirmDisconnectOpen = true)}
            disabled={actionBusy || scanning}
            class="px-3 py-1.5 text-xs font-medium rounded-md border border-status-warn-border bg-status-warn-bg text-status-warn cursor-pointer hover:opacity-80 disabled:opacity-40 disabled:cursor-not-allowed"
          >
            Disconnect
          </button>
        {/if}
      </div>
    </Card>

    <!-- Available Networks Card -->
    <Card>
      {#snippet header()}
        <h2 class="text-sm font-semibold text-text-primary m-0">
          Available Networks
        </h2>
        <div class="ml-auto">
          <button
            type="button"
            data-testid="wifi-scan-btn"
            onclick={() => handleScan(true)}
            disabled={scanning || actionBusy}
            class="px-3 py-1 text-xs font-medium rounded-md bg-accent text-white border-none cursor-pointer hover:opacity-90 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {scanning ? "Scanning\u2026" : "Scan for Networks"}
          </button>
        </div>
      {/snippet}

      {#if networks.length === 0}
        <p class="text-sm text-text-muted m-0">
          {scanning
            ? "Scanning for nearby Wi-Fi networks\u2026"
            : "No networks found. Click Scan for Networks to search nearby access points."}
        </p>
      {:else}
        <div class="divide-y divide-border">
          {#each networks as net (net.ssid)}
            {@const bars = signalBars(net.signal)}
            {@const secured = isNetworkSecured(net.security)}
            <div class="py-2.5 first:pt-0 last:pb-0">
              <div class="flex flex-wrap items-center justify-between gap-2">
                <div class="flex items-center gap-2.5 min-w-0">
                  <!-- 4-bar signal indicator -->
                  <span
                    class="inline-flex items-end gap-0.5 h-3.5 shrink-0"
                    title="{net.signal}%"
                    aria-label="Signal {net.signal}%"
                  >
                    {#each [1, 2, 3, 4] as bar (bar)}
                      <span
                        class="w-1 rounded-xs {bar <= bars
                          ? 'bg-status-ok'
                          : 'bg-surface-3'}"
                        style="height: {bar * 25}%"
                      ></span>
                    {/each}
                  </span>

                  <div class="min-w-0">
                    <div class="flex items-center gap-2 flex-wrap">
                      <span class="font-medium text-sm text-text-primary truncate">
                        {net.ssid}
                      </span>
                      {#if net.in_use}
                        <StatusBadge label="Connected" state="ok" />
                      {:else if net.saved}
                        <span
                          class="text-[11px] font-medium px-1.5 py-0.5 rounded bg-surface-2 text-text-secondary border border-border"
                        >
                          Saved
                        </span>
                      {/if}
                    </div>
                    <div class="text-xs text-text-muted flex items-center gap-2 mt-0.5">
                      <span>{secured ? net.security : "Open"}</span>
                      {#if net.bands.length > 0}
                        <span>&middot;</span>
                        <span>{net.bands.join(" / ")}</span>
                      {/if}
                      <span>&middot;</span>
                      <span class="font-mono">{net.signal}%</span>
                    </div>
                  </div>
                </div>

                <div class="flex items-center gap-2">
                  {#if net.in_use}
                    <span class="text-xs text-text-muted">Active</span>
                  {:else}
                    {#if net.saved && secured}
                      <button
                        type="button"
                        onclick={() => startConnectRow(net, true)}
                        disabled={actionBusy || scanning}
                        class="px-2.5 py-1 text-xs rounded-md border border-border bg-surface-2 text-text-secondary cursor-pointer hover:bg-surface-3 disabled:opacity-40"
                      >
                        New Password
                      </button>
                    {/if}
                    <button
                      type="button"
                      data-testid="wifi-connect-{net.ssid}"
                      onclick={() => startConnectRow(net, false)}
                      disabled={actionBusy || scanning}
                      class="px-3 py-1 text-xs font-medium rounded-md border border-border bg-surface-2 text-text-primary cursor-pointer hover:bg-surface-3 disabled:opacity-40"
                    >
                      Connect
                    </button>
                  {/if}
                </div>
              </div>

              {#if selectedSsid === net.ssid}
                <form
                  class="mt-2.5 p-3 rounded-md bg-surface-2 border border-border flex flex-wrap items-center gap-2"
                  onsubmit={(e) => {
                    e.preventDefault();
                    void executeConnect({
                      ssid: net.ssid,
                      password: selectedPassword,
                    });
                  }}
                >
                  <label class="sr-only" for="wifi-pass-{net.ssid}">
                    Password for {net.ssid}
                  </label>
                  <input
                    id="wifi-pass-{net.ssid}"
                    data-testid="wifi-password-input"
                    type={showPassword ? "text" : "password"}
                    placeholder="Wi-Fi password for {net.ssid}"
                    bind:value={selectedPassword}
                    required
                    class="{inputClass} flex-1 min-w-[200px]"
                  />
                  <button
                    type="button"
                    onclick={() => (showPassword = !showPassword)}
                    class="px-2.5 py-1.5 text-xs rounded-md border border-border bg-surface-1 text-text-secondary cursor-pointer hover:bg-surface-3"
                  >
                    {showPassword ? "Hide" : "Show"}
                  </button>
                  <button
                    type="submit"
                    data-testid="wifi-join-submit"
                    disabled={actionBusy || !selectedPassword}
                    class="px-3 py-1.5 text-xs font-medium rounded-md bg-accent text-white border-none cursor-pointer hover:opacity-90 disabled:opacity-50"
                  >
                    {actionBusy ? "Connecting\u2026" : "Join"}
                  </button>
                  <button
                    type="button"
                    onclick={cancelConnectRow}
                    disabled={actionBusy}
                    class="px-2.5 py-1.5 text-xs rounded-md border border-border bg-surface-1 text-text-secondary cursor-pointer hover:bg-surface-3"
                  >
                    Cancel
                  </button>
                </form>
              {/if}
            </div>
          {/each}
        </div>
      {/if}

      <!-- Collapsible Join Hidden Network Section -->
      <div class="mt-4 pt-3 border-t border-border">
        <button
          type="button"
          data-testid="wifi-hidden-toggle"
          onclick={() => (showHiddenForm = !showHiddenForm)}
          class="text-xs font-medium text-accent bg-transparent border-none p-0 cursor-pointer hover:underline"
        >
          {showHiddenForm ? "Hide Hidden Network Form" : "+ Join Hidden Network"}
        </button>

        {#if showHiddenForm}
          <form
            onsubmit={handleHiddenSubmit}
            class="mt-3 p-3 rounded-md bg-surface-2 border border-border grid grid-cols-1 sm:grid-cols-2 gap-3"
          >
            <div>
              <label
                for="hidden-ssid"
                class="block text-xs text-text-secondary mb-1"
              >
                Network Name (SSID)
              </label>
              <input
                id="hidden-ssid"
                data-testid="wifi-hidden-ssid"
                type="text"
                bind:value={hiddenSsid}
                placeholder="Hidden SSID"
                required
                class="{inputClass} w-full"
              />
            </div>

            <div>
              <label
                for="hidden-security"
                class="block text-xs text-text-secondary mb-1"
              >
                Security
              </label>
              <select
                id="hidden-security"
                bind:value={hiddenSecurity}
                class="{inputClass} w-full"
              >
                <option value="wpa">WPA / WPA2 / WPA3 Personal</option>
                <option value="open">Open (No Password)</option>
              </select>
            </div>

            {#if hiddenSecurity === "wpa"}
              <div class="sm:col-span-2 flex items-end gap-2">
                <div class="flex-1">
                  <label
                    for="hidden-password"
                    class="block text-xs text-text-secondary mb-1"
                  >
                    Password
                  </label>
                  <input
                    id="hidden-password"
                    data-testid="wifi-hidden-password"
                    type={showHiddenPassword ? "text" : "password"}
                    bind:value={hiddenPassword}
                    placeholder="Network password"
                    required
                    class="{inputClass} w-full"
                  />
                </div>
                <button
                  type="button"
                  onclick={() => (showHiddenPassword = !showHiddenPassword)}
                  class="px-2.5 py-1.5 text-xs rounded-md border border-border bg-surface-1 text-text-secondary cursor-pointer hover:bg-surface-3"
                >
                  {showHiddenPassword ? "Hide" : "Show"}
                </button>
              </div>
            {/if}

            <div class="sm:col-span-2 flex justify-end gap-2">
              <button
                type="button"
                onclick={() => (showHiddenForm = false)}
                class="px-3 py-1.5 text-xs rounded-md border border-border bg-surface-1 text-text-secondary cursor-pointer hover:bg-surface-3"
              >
                Cancel
              </button>
              <button
                type="submit"
                data-testid="wifi-hidden-submit"
                disabled={actionBusy}
                class="px-3 py-1.5 text-xs font-medium rounded-md bg-accent text-white border-none cursor-pointer hover:opacity-90 disabled:opacity-50"
              >
                {actionBusy ? "Connecting\u2026" : "Connect"}
              </button>
            </div>
          </form>
        {/if}
      </div>
    </Card>

    <!-- Saved Networks Card -->
    <Card title="Saved Wi-Fi Profiles">
      {#if status.saved_networks.length === 0}
        <p class="text-sm text-text-muted m-0">
          No saved Wi-Fi connection profiles on this forwarder.
        </p>
      {:else}
        <div class="divide-y divide-border">
          {#each status.saved_networks as saved (saved.uuid)}
            <div
              class="py-2 first:pt-0 last:pb-0 flex items-center justify-between gap-2"
            >
              <div class="flex items-center gap-2 min-w-0">
                <span class="text-sm font-medium text-text-primary truncate">
                  {saved.ssid}
                </span>
                {#if status.ssid === saved.ssid && status.state === "connected"}
                  <StatusBadge label="Active" state="ok" />
                {/if}
                {#if saved.autoconnect}
                  <span class="text-xs text-text-muted">(autoconnect)</span>
                {/if}
              </div>
              <button
                type="button"
                data-testid="wifi-forget-{saved.ssid}"
                onclick={() => (confirmForgetSsid = saved.ssid)}
                disabled={actionBusy || scanning}
                class="px-2.5 py-1 text-xs font-medium rounded-md border border-status-err-border bg-status-err-bg text-status-err cursor-pointer hover:opacity-80 disabled:opacity-40"
              >
                Forget
              </button>
            </div>
          {/each}
        </div>
      {/if}
    </Card>
  {/if}
</div>

<ConfirmDialog
  open={confirmDisconnectOpen}
  title="Disconnect Wi-Fi"
  message={isRemoteP2p
    ? "Are you sure you want to disconnect the forwarder's Wi-Fi? If the forwarder uses Wi-Fi for its P2P uplink, it will go offline until Ethernet is connected or it re-associates."
    : "Are you sure you want to disconnect the forwarder's Wi-Fi? If you are accessing this web UI over Wi-Fi, your browser connection will drop immediately."}
  confirmLabel="Disconnect"
  variant="warn"
  busy={actionBusy}
  onConfirm={handleConfirmDisconnect}
  onCancel={() => (confirmDisconnectOpen = false)}
/>

<ConfirmDialog
  open={confirmForgetSsid !== null}
  title="Forget Saved Wi-Fi Network"
  message={confirmForgetSsid &&
  status?.ssid === confirmForgetSsid &&
  status?.state === "connected"
    ? `Are you sure you want to forget "${confirmForgetSsid}"? This is the currently active Wi-Fi network; forgetting it will disconnect the forwarder from "${confirmForgetSsid}".`
    : `Are you sure you want to delete the saved Wi-Fi profile for "${confirmForgetSsid ?? ""}"?`}
  confirmLabel="Forget"
  variant="err"
  busy={actionBusy}
  onConfirm={handleConfirmForget}
  onCancel={() => (confirmForgetSsid = null)}
/>
