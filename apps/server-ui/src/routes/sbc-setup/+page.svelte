<script lang="ts">
  import { onMount } from "svelte";
  import { Card } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";
  import type { SbcSetupFormData } from "$lib/sbc-setup/types";
  import {
    validateHostname,
    validateUsername,
    validateSshKey,
    validateIpv4Cidr,
    validateIpv4Address,
    parseDnsServers,
    validateBaseUrl,
    parseReaderTargets,
    validateWifiCountry,
  } from "$lib/sbc-setup/validation";
  import {
    generateUserData,
    generateNetworkConfig,
  } from "$lib/sbc-setup/generate";
  import {
    readSbcSetupPreference,
    writeSbcSetupPreference,
    autoIncrement,
    computeBaseOctet,
  } from "$lib/sbc-setup/persistence";
  import { downloadFile } from "$lib/sbc-setup/download";

  const DEFAULTS: SbcSetupFormData = {
    hostname: "rt-fwd-01",
    adminUsername: "rt-admin",
    sshPublicKey: "",
    staticIpv4Cidr: "192.168.1.51/24",
    gateway: "192.168.1.1",
    dnsServers: "8.8.8.8,8.8.4.4",
    wifiEnabled: false,
    wifiSsid: "",
    wifiPassword: "",
    wifiCountry: "US",
    serverBaseUrl: "",
    authToken: "",
    readerTargets: "",
    statusBind: "0.0.0.0:80",
    displayName: "",
  };

  let form: SbcSetupFormData = $state({ ...DEFAULTS });
  let ipBaseOctet = $state(0);
  let errors: Record<string, string> = $state({});
  let tokenCreating = $state(false);
  let tokenError = $state("");
  let feedback = $state("");

  onMount(() => {
    const saved = readSbcSetupPreference();
    if (saved) {
      form = { ...saved };
      ipBaseOctet = saved.ipBaseOctet;
    } else {
      ipBaseOctet = computeBaseOctet(
        DEFAULTS.hostname,
        DEFAULTS.staticIpv4Cidr,
      );
    }
  });

  function handleBlur(field: string) {
    const newErrors = { ...errors };
    delete newErrors[field];

    switch (field) {
      case "hostname": {
        const r = validateHostname(form.hostname);
        if (r instanceof Error) newErrors.hostname = r.message;
        break;
      }
      case "adminUsername": {
        const r = validateUsername(form.adminUsername);
        if (r instanceof Error) newErrors.adminUsername = r.message;
        break;
      }
      case "sshPublicKey": {
        const r = validateSshKey(form.sshPublicKey);
        if (r instanceof Error) newErrors.sshPublicKey = r.message;
        break;
      }
      case "staticIpv4Cidr": {
        const r = validateIpv4Cidr(form.staticIpv4Cidr);
        if (r instanceof Error) newErrors.staticIpv4Cidr = r.message;
        break;
      }
      case "gateway": {
        const r = validateIpv4Address(form.gateway);
        if (r instanceof Error) newErrors.gateway = r.message;
        break;
      }
      case "dnsServers": {
        const r = parseDnsServers(form.dnsServers);
        if (r instanceof Error) newErrors.dnsServers = r.message;
        break;
      }
      case "wifiSsid": {
        if (form.wifiEnabled && !form.wifiSsid.trim())
          newErrors.wifiSsid = "Wi-Fi SSID is required when Wi-Fi is enabled";
        break;
      }
      case "wifiCountry": {
        if (form.wifiEnabled) {
          const r = validateWifiCountry(form.wifiCountry);
          if (r instanceof Error) newErrors.wifiCountry = r.message;
        }
        break;
      }
      case "serverBaseUrl": {
        const r = validateBaseUrl(form.serverBaseUrl);
        if (r instanceof Error) newErrors.serverBaseUrl = r.message;
        break;
      }
      case "readerTargets": {
        const r = parseReaderTargets(form.readerTargets);
        if (r instanceof Error) newErrors.readerTargets = r.message;
        break;
      }
    }

    errors = newErrors;
  }

  function validateAll(): boolean {
    const newErrors: Record<string, string> = {};

    const hostname = validateHostname(form.hostname);
    if (hostname instanceof Error) newErrors.hostname = hostname.message;

    const username = validateUsername(form.adminUsername);
    if (username instanceof Error) newErrors.adminUsername = username.message;

    const sshKey = validateSshKey(form.sshPublicKey);
    if (sshKey instanceof Error) newErrors.sshPublicKey = sshKey.message;

    const cidr = validateIpv4Cidr(form.staticIpv4Cidr);
    if (cidr instanceof Error) newErrors.staticIpv4Cidr = cidr.message;

    const gw = validateIpv4Address(form.gateway);
    if (gw instanceof Error) newErrors.gateway = gw.message;

    const dns = parseDnsServers(form.dnsServers);
    if (dns instanceof Error) newErrors.dnsServers = dns.message;

    if (form.wifiEnabled) {
      if (!form.wifiSsid.trim())
        newErrors.wifiSsid = "Wi-Fi SSID is required when Wi-Fi is enabled";
      const country = validateWifiCountry(form.wifiCountry);
      if (country instanceof Error) newErrors.wifiCountry = country.message;
    }

    const baseUrl = validateBaseUrl(form.serverBaseUrl);
    if (baseUrl instanceof Error) newErrors.serverBaseUrl = baseUrl.message;

    const targets = parseReaderTargets(form.readerTargets);
    if (targets instanceof Error) newErrors.readerTargets = targets.message;

    errors = newErrors;
    return Object.keys(newErrors).length === 0;
  }

  function handleDownloadUserData() {
    if (!validateAll()) return;
    const content = generateUserData(form);
    downloadFile("user-data", content);
  }

  function handleDownloadNetworkConfig() {
    if (!validateAll()) return;
    const content = generateNetworkConfig(form);
    downloadFile("network-config", content);
  }

  function handleDownloadBoth() {
    if (!validateAll()) return;

    const userData = generateUserData(form);
    const networkConfig = generateNetworkConfig(form);

    downloadFile("user-data", userData);
    downloadFile("network-config", networkConfig);

    writeSbcSetupPreference({ ...form, ipBaseOctet });

    const incremented = autoIncrement({
      hostname: form.hostname,
      staticIpv4Cidr: form.staticIpv4Cidr,
      ipBaseOctet,
    });
    form.hostname = incremented.hostname;
    form.staticIpv4Cidr = incremented.staticIpv4Cidr;
    ipBaseOctet = incremented.ipBaseOctet;
    form.authToken = "";
    form.displayName = "";

    feedback = "Downloaded! Form auto-incremented for next device.";
  }

  async function handleCreateToken() {
    tokenCreating = true;
    tokenError = "";
    try {
      const result = await api.createToken({
        device_id: form.hostname,
        device_type: "forwarder",
      });
      form.authToken = result.token;
    } catch (e) {
      tokenError = String(e);
    } finally {
      tokenCreating = false;
    }
  }
</script>

<svelte:head>
  <title>SBC Setup &middot; Rusty Timer</title>
</svelte:head>

<main class="max-w-[1100px] mx-auto px-6 py-6">
  <div class="flex items-center justify-between mb-6">
    <h1 class="text-xl font-bold text-text-primary m-0">SBC Setup</h1>
  </div>

  {#if feedback}
    <p class="text-sm mb-4 m-0 text-status-ok">{feedback}</p>
  {/if}

  <!-- Device Identity -->
  <div class="mb-6">
    <Card title="Device Identity">
      <details open>
        <summary
          class="text-sm font-medium text-text-secondary cursor-pointer mb-3"
        >
          Hostname, admin user, and SSH key
        </summary>
        <div class="flex flex-col gap-4">
          <!-- Hostname -->
          <div>
            <label for="hostname" class="block">
              <span class="text-sm text-text-muted">Hostname</span>
            </label>
            <input
              id="hostname"
              type="text"
              bind:value={form.hostname}
              onblur={() => handleBlur("hostname")}
              class="mt-1 block w-full rounded border px-3 py-2 text-sm bg-surface-0 text-text-primary {errors.hostname
                ? 'border-status-err'
                : 'border-border'}"
            />
            {#if errors.hostname}
              <p class="text-xs text-status-err mt-1 m-0">{errors.hostname}</p>
            {/if}
          </div>

          <!-- Admin Username -->
          <div>
            <label for="adminUsername" class="block">
              <span class="text-sm text-text-muted">Admin Username</span>
            </label>
            <input
              id="adminUsername"
              type="text"
              bind:value={form.adminUsername}
              onblur={() => handleBlur("adminUsername")}
              class="mt-1 block w-full rounded border px-3 py-2 text-sm bg-surface-0 text-text-primary {errors.adminUsername
                ? 'border-status-err'
                : 'border-border'}"
            />
            {#if errors.adminUsername}
              <p class="text-xs text-status-err mt-1 m-0">
                {errors.adminUsername}
              </p>
            {/if}
          </div>

          <!-- SSH Public Key -->
          <div>
            <label for="sshPublicKey" class="block">
              <span class="text-sm text-text-muted">SSH Public Key</span>
            </label>
            <textarea
              id="sshPublicKey"
              bind:value={form.sshPublicKey}
              onblur={() => handleBlur("sshPublicKey")}
              rows="3"
              class="mt-1 block w-full rounded border px-3 py-2 text-sm bg-surface-0 text-text-primary font-mono {errors.sshPublicKey
                ? 'border-status-err'
                : 'border-border'}"
            ></textarea>
            {#if errors.sshPublicKey}
              <p class="text-xs text-status-err mt-1 m-0">
                {errors.sshPublicKey}
              </p>
            {/if}
          </div>
        </div>
      </details>
    </Card>
  </div>

  <!-- Network Configuration -->
  <div class="mb-6">
    <Card title="Network Configuration">
      <details open>
        <summary
          class="text-sm font-medium text-text-secondary cursor-pointer mb-3"
        >
          Static IP, gateway, DNS, and Wi-Fi
        </summary>
        <div class="flex flex-col gap-4">
          <!-- Static IPv4/CIDR -->
          <div>
            <label for="staticIpv4Cidr" class="block">
              <span class="text-sm text-text-muted">Static IPv4/CIDR</span>
            </label>
            <input
              id="staticIpv4Cidr"
              type="text"
              bind:value={form.staticIpv4Cidr}
              onblur={() => handleBlur("staticIpv4Cidr")}
              placeholder="192.168.1.51/24"
              class="mt-1 block w-full rounded border px-3 py-2 text-sm bg-surface-0 text-text-primary {errors.staticIpv4Cidr
                ? 'border-status-err'
                : 'border-border'}"
            />
            {#if errors.staticIpv4Cidr}
              <p class="text-xs text-status-err mt-1 m-0">
                {errors.staticIpv4Cidr}
              </p>
            {/if}
          </div>

          <!-- Gateway -->
          <div>
            <label for="gateway" class="block">
              <span class="text-sm text-text-muted">Gateway</span>
            </label>
            <input
              id="gateway"
              type="text"
              bind:value={form.gateway}
              onblur={() => handleBlur("gateway")}
              placeholder="192.168.1.1"
              class="mt-1 block w-full rounded border px-3 py-2 text-sm bg-surface-0 text-text-primary {errors.gateway
                ? 'border-status-err'
                : 'border-border'}"
            />
            {#if errors.gateway}
              <p class="text-xs text-status-err mt-1 m-0">{errors.gateway}</p>
            {/if}
          </div>

          <!-- DNS Servers -->
          <div>
            <label for="dnsServers" class="block">
              <span class="text-sm text-text-muted"
                >DNS Servers (comma-separated)</span
              >
            </label>
            <input
              id="dnsServers"
              type="text"
              bind:value={form.dnsServers}
              onblur={() => handleBlur("dnsServers")}
              placeholder="8.8.8.8,8.8.4.4"
              class="mt-1 block w-full rounded border px-3 py-2 text-sm bg-surface-0 text-text-primary {errors.dnsServers
                ? 'border-status-err'
                : 'border-border'}"
            />
            {#if errors.dnsServers}
              <p class="text-xs text-status-err mt-1 m-0">
                {errors.dnsServers}
              </p>
            {/if}
          </div>

          <!-- Wi-Fi Toggle -->
          <div>
            <label class="flex items-center gap-2 cursor-pointer">
              <input
                type="checkbox"
                bind:checked={form.wifiEnabled}
                class="accent-accent"
              />
              <span class="text-sm text-text-muted">Enable Wi-Fi</span>
            </label>
          </div>

          {#if form.wifiEnabled}
            <!-- Wi-Fi SSID -->
            <div>
              <label for="wifiSsid" class="block">
                <span class="text-sm text-text-muted">Wi-Fi SSID</span>
              </label>
              <input
                id="wifiSsid"
                type="text"
                bind:value={form.wifiSsid}
                onblur={() => handleBlur("wifiSsid")}
                class="mt-1 block w-full rounded border px-3 py-2 text-sm bg-surface-0 text-text-primary {errors.wifiSsid
                  ? 'border-status-err'
                  : 'border-border'}"
              />
              {#if errors.wifiSsid}
                <p class="text-xs text-status-err mt-1 m-0">
                  {errors.wifiSsid}
                </p>
              {/if}
            </div>

            <!-- Wi-Fi Password -->
            <div>
              <label for="wifiPassword" class="block">
                <span class="text-sm text-text-muted">Wi-Fi Password</span>
              </label>
              <input
                id="wifiPassword"
                type="password"
                bind:value={form.wifiPassword}
                class="mt-1 block w-full rounded border px-3 py-2 text-sm bg-surface-0 text-text-primary border-border"
              />
            </div>

            <!-- Wi-Fi Country -->
            <div>
              <label for="wifiCountry" class="block">
                <span class="text-sm text-text-muted">Wi-Fi Country Code</span>
              </label>
              <input
                id="wifiCountry"
                type="text"
                bind:value={form.wifiCountry}
                onblur={() => handleBlur("wifiCountry")}
                placeholder="US"
                class="mt-1 block w-full rounded border px-3 py-2 text-sm bg-surface-0 text-text-primary {errors.wifiCountry
                  ? 'border-status-err'
                  : 'border-border'}"
              />
              {#if errors.wifiCountry}
                <p class="text-xs text-status-err mt-1 m-0">
                  {errors.wifiCountry}
                </p>
              {/if}
            </div>
          {/if}
        </div>
      </details>
    </Card>
  </div>

  <!-- Forwarder Setup -->
  <div class="mb-6">
    <Card title="Forwarder Setup">
      <details open>
        <summary
          class="text-sm font-medium text-text-secondary cursor-pointer mb-3"
        >
          Server connection, reader targets, and display name
        </summary>
        <div class="flex flex-col gap-4">
          <!-- Server Base URL -->
          <div>
            <label for="serverBaseUrl" class="block">
              <span class="text-sm text-text-muted">Server Base URL</span>
            </label>
            <input
              id="serverBaseUrl"
              type="text"
              bind:value={form.serverBaseUrl}
              onblur={() => handleBlur("serverBaseUrl")}
              placeholder="https://timer.example.com"
              class="mt-1 block w-full rounded border px-3 py-2 text-sm bg-surface-0 text-text-primary {errors.serverBaseUrl
                ? 'border-status-err'
                : 'border-border'}"
            />
            {#if errors.serverBaseUrl}
              <p class="text-xs text-status-err mt-1 m-0">
                {errors.serverBaseUrl}
              </p>
            {/if}
          </div>

          <!-- Auth Token -->
          <div>
            <label for="authToken" class="block">
              <span class="text-sm text-text-muted">Auth Token</span>
            </label>
            <div class="flex gap-2 mt-1">
              <input
                id="authToken"
                type="text"
                bind:value={form.authToken}
                class="block flex-1 rounded border px-3 py-2 text-sm bg-surface-0 text-text-primary border-border font-mono"
              />
              <button
                onclick={handleCreateToken}
                disabled={tokenCreating || !form.hostname.trim()}
                class="px-4 py-2 text-sm font-medium rounded-md bg-surface-1 text-text-primary border border-border cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {tokenCreating ? "Creating..." : "Create Token"}
              </button>
            </div>
            {#if tokenError}
              <p class="text-xs text-status-err mt-1 m-0">{tokenError}</p>
            {/if}
          </div>

          <!-- Reader Targets -->
          <div>
            <label for="readerTargets" class="block">
              <span class="text-sm text-text-muted"
                >Reader Targets (one per line, IP:PORT)</span
              >
            </label>
            <textarea
              id="readerTargets"
              bind:value={form.readerTargets}
              onblur={() => handleBlur("readerTargets")}
              rows="3"
              placeholder="192.168.1.10:10000"
              class="mt-1 block w-full rounded border px-3 py-2 text-sm bg-surface-0 text-text-primary font-mono {errors.readerTargets
                ? 'border-status-err'
                : 'border-border'}"
            ></textarea>
            {#if errors.readerTargets}
              <p class="text-xs text-status-err mt-1 m-0">
                {errors.readerTargets}
              </p>
            {/if}
          </div>

          <!-- Status Bind -->
          <div>
            <label for="statusBind" class="block">
              <span class="text-sm text-text-muted">Status Bind Address</span>
            </label>
            <input
              id="statusBind"
              type="text"
              bind:value={form.statusBind}
              placeholder="0.0.0.0:80"
              class="mt-1 block w-full rounded border px-3 py-2 text-sm bg-surface-0 text-text-primary border-border"
            />
          </div>

          <!-- Display Name -->
          <div>
            <label for="displayName" class="block">
              <span class="text-sm text-text-muted"
                >Display Name (optional)</span
              >
            </label>
            <input
              id="displayName"
              type="text"
              bind:value={form.displayName}
              placeholder="Start Line"
              class="mt-1 block w-full rounded border px-3 py-2 text-sm bg-surface-0 text-text-primary border-border"
            />
          </div>
        </div>
      </details>
    </Card>
  </div>

  <!-- Action Buttons -->
  <div class="flex flex-wrap gap-3">
    <button
      onclick={handleDownloadBoth}
      class="px-4 py-2 text-sm font-medium rounded-md bg-accent text-white border-none cursor-pointer hover:opacity-80"
    >
      Download Both &amp; Next
    </button>
    <button
      onclick={handleDownloadUserData}
      class="px-4 py-2 text-sm font-medium rounded-md bg-surface-1 text-text-primary border border-border cursor-pointer hover:opacity-80"
    >
      Download user-data
    </button>
    <button
      onclick={handleDownloadNetworkConfig}
      class="px-4 py-2 text-sm font-medium rounded-md bg-surface-1 text-text-primary border border-border cursor-pointer hover:opacity-80"
    >
      Download network-config
    </button>
  </div>
</main>
