<script lang="ts">
  import { onMount } from "svelte";
  import {
    AlertBanner,
    Card,
    HelpTip,
    StatusBadge,
    buttonClass,
    inputClass,
    tableCellClass,
    tableClass,
    tableHeadRowClass,
    tableHeaderCellClass,
    tableRowClass,
  } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";
  import type {
    CreateEnrollmentTokenResponse,
    EnrollmentTokenRecord,
  } from "$lib/api";
  import { downloadFile } from "$lib/sbc-setup/download";
  import {
    DEFAULT_SETUP_SCRIPT_URL,
    generateNetworkConfig,
    generateUserData,
  } from "$lib/sbc-setup/generate";
  import {
    autoIncrement,
    computeBaseOctet,
    readSbcSetupPreference,
    writeSbcSetupPreference,
  } from "$lib/sbc-setup/persistence";
  import type { SbcSetupFormData } from "$lib/sbc-setup/types";
  import {
    parseDnsServers,
    parseReaderTargets,
    validateBaseUrl,
    validateHostname,
    validateIpv4Address,
    validateIpv4Cidr,
    validateSshKey,
    validateStatusBind,
    validateUsername,
    validateWifiCountry,
  } from "$lib/sbc-setup/validation";

  const DEFAULT_FORM: SbcSetupFormData = {
    hostname: "rt-fwd-01",
    adminUsername: "rt-admin",
    sshPublicKey: "",
    staticIpv4Cidr: "192.168.1.50/24",
    gateway: "192.168.1.1",
    dnsServers: "8.8.8.8,8.8.4.4",
    wifiEnabled: false,
    wifiSsid: "",
    wifiPassword: "",
    wifiCountry: "US",
    serverUrl: "",
    authToken: "",
    readerTargets: "192.168.1.10:10000",
    statusBind: "0.0.0.0:80",
    displayName: "Start Line",
    setupScriptUrl: DEFAULT_SETUP_SCRIPT_URL,
    upsEnabled: false,
  };

  let tokens = $state<EnrollmentTokenRecord[]>([]);
  // SBC Setup only provisions forwarders; show only forwarder tokens so receiver
  // tokens (created from the Admin page) do not bleed into this table.
  let forwarderTokens = $derived(
    tokens.filter((token) => token.device_kind === "forwarder"),
  );
  let tokensLoading = $state(true);
  let tokenBusy = $state<string | null>(null);
  let tokenError = $state<string | null>(null);
  let tokenSuccess = $state<string | null>(null);
  let createdToken = $state<CreateEnrollmentTokenResponse | null>(null);
  let createDisplayName = $state("");
  let manualToken = $state("");

  let form = $state<SbcSetupFormData>({ ...DEFAULT_FORM });
  let ipBaseOctet = $state(50);
  let formError = $state<string | null>(null);
  let formSuccess = $state<string | null>(null);

  function endpointShort(endpointId: string | null) {
    if (!endpointId) return "—";
    return endpointId.length > 18 ? `${endpointId.slice(0, 18)}…` : endpointId;
  }

  function formatTime(value: number | null) {
    if (value == null) return "—";
    return new Date(value).toLocaleString();
  }

  function tokenState(status: EnrollmentTokenRecord["status"]) {
    if (status === "active") return "ok";
    if (status === "used") return "warn";
    if (status === "expired") return "err";
    return "err";
  }

  function setTokenMessage(message: string) {
    tokenSuccess = message;
    tokenError = null;
  }

  async function loadTokens() {
    tokensLoading = true;
    try {
      tokens = (await api.listEnrollmentTokens()).tokens;
      tokenError = null;
    } catch (err) {
      tokenError = String(err);
    } finally {
      tokensLoading = false;
    }
  }

  async function createToken(useManualToken: boolean) {
    tokenBusy = useManualToken ? "manual" : "generate";
    tokenError = null;
    tokenSuccess = null;
    createdToken = null;
    const trimmedDisplayName = createDisplayName.trim();
    const trimmedManualToken = manualToken.trim();

    if (useManualToken && !trimmedManualToken) {
      tokenBusy = null;
      tokenError = "Manual token is required.";
      return;
    }

    try {
      const response = await api.createEnrollmentToken({
        device_kind: "forwarder",
        display_name: trimmedDisplayName || undefined,
        token: useManualToken ? trimmedManualToken : undefined,
      });
      createdToken = response;
      form.authToken = response.token;
      if (response.display_name) {
        form.displayName = response.display_name;
      }
      manualToken = "";
      setTokenMessage(
        "Token created. Copy it now or use it in the setup form below.",
      );
      await loadTokens();
    } catch (err) {
      tokenError = String(err);
    } finally {
      tokenBusy = null;
    }
  }

  async function revokeToken(token: EnrollmentTokenRecord) {
    tokenBusy = token.token_id;
    tokenError = null;
    tokenSuccess = null;
    try {
      await api.revokeEnrollmentToken(token.token_id);
      setTokenMessage(`Revoked ${token.token_id}.`);
      await loadTokens();
    } catch (err) {
      tokenError = String(err);
    } finally {
      tokenBusy = null;
    }
  }

  function useCreatedToken() {
    if (!createdToken) return;
    form.authToken = createdToken.token;
    if (createdToken.display_name) {
      form.displayName = createdToken.display_name;
    }
    setTokenMessage("One-time token copied into the setup form.");
  }

  function pushValidation(result: string | string[] | Error, errors: string[]) {
    if (result instanceof Error) errors.push(result.message);
  }

  function validateForm() {
    const errors: string[] = [];
    pushValidation(validateHostname(form.hostname), errors);
    pushValidation(validateUsername(form.adminUsername), errors);
    pushValidation(validateSshKey(form.sshPublicKey), errors);
    pushValidation(validateIpv4Cidr(form.staticIpv4Cidr), errors);
    pushValidation(validateIpv4Address(form.gateway), errors);
    pushValidation(parseDnsServers(form.dnsServers), errors);
    pushValidation(validateBaseUrl(form.serverUrl), errors);
    if (!form.authToken.trim()) errors.push("Auth token is required.");
    pushValidation(parseReaderTargets(form.readerTargets), errors);
    pushValidation(validateStatusBind(form.statusBind), errors);
    if (!form.displayName.trim()) errors.push("Display name is required.");
    pushValidation(validateBaseUrl(form.setupScriptUrl), errors);

    if (form.wifiEnabled) {
      if (!form.wifiSsid.trim())
        errors.push("Wi-Fi SSID is required when Wi-Fi is enabled.");
      pushValidation(validateWifiCountry(form.wifiCountry), errors);
    }

    return errors;
  }

  function validatedForm() {
    const errors = validateForm();
    if (errors.length > 0) {
      formError = errors.join(" ");
      formSuccess = null;
      return null;
    }

    formError = null;
    return {
      ...form,
      hostname: form.hostname.trim(),
      adminUsername: form.adminUsername.trim(),
      sshPublicKey: form.sshPublicKey.trim(),
      staticIpv4Cidr: form.staticIpv4Cidr.trim(),
      gateway: form.gateway.trim(),
      dnsServers: form.dnsServers.trim(),
      wifiSsid: form.wifiSsid.trim(),
      wifiPassword: form.wifiPassword,
      wifiCountry: (form.wifiCountry || "US").trim().toUpperCase(),
      serverUrl: form.serverUrl.trim(),
      authToken: form.authToken.trim(),
      readerTargets: form.readerTargets.trim(),
      statusBind: form.statusBind.trim(),
      displayName: form.displayName.trim(),
      setupScriptUrl: form.setupScriptUrl.trim() || DEFAULT_SETUP_SCRIPT_URL,
    };
  }

  function downloadUserData() {
    const config = validatedForm();
    if (!config) return;
    downloadFile("user-data", generateUserData(config));
    formSuccess = "Downloaded user-data.";
  }

  function downloadNetworkConfig() {
    const config = validatedForm();
    if (!config) return;
    downloadFile("network-config", generateNetworkConfig(config));
    formSuccess = "Downloaded network-config.";
  }

  function saveAndNextDevice() {
    const currentBaseOctet =
      computeBaseOctet(form.hostname, form.staticIpv4Cidr) || ipBaseOctet;
    writeSbcSetupPreference({ form, ipBaseOctet: currentBaseOctet });
    const next = autoIncrement({
      hostname: form.hostname,
      staticIpv4Cidr: form.staticIpv4Cidr,
      ipBaseOctet: currentBaseOctet,
    });
    form.hostname = next.hostname;
    form.staticIpv4Cidr = next.staticIpv4Cidr;
    form.authToken = "";
    formError = null;
    formSuccess =
      "Saved non-secret preferences and prepared the next device. Add or generate a new token before downloading.";
    ipBaseOctet = next.ipBaseOctet;
  }

  onMount(() => {
    const stored = readSbcSetupPreference();
    if (stored) {
      form = {
        ...DEFAULT_FORM,
        ...stored.form,
        serverUrl: stored.form.serverUrl || window.location.origin,
        setupScriptUrl: stored.form.setupScriptUrl || DEFAULT_SETUP_SCRIPT_URL,
      };
      ipBaseOctet =
        stored.ipBaseOctet ||
        computeBaseOctet(form.hostname, form.staticIpv4Cidr);
    } else {
      form.serverUrl = window.location.origin;
    }
    void loadTokens();
  });
</script>

<svelte:head>
  <title>SBC Setup · Rusty Timer Server</title>
</svelte:head>

<div class="mx-auto px-4 py-6 space-y-6" style="max-width: 1180px;">
  <div class="flex flex-wrap items-center justify-between gap-3">
    <div>
      <h1 class="text-2xl font-bold text-text-primary m-0">SBC Setup</h1>
      <p class="text-sm text-text-muted mt-1 mb-0">
        Create forwarder enrollment tokens and generate Raspberry Pi cloud-init
        files for first boot.
      </p>
    </div>
    {#if tokensLoading}
      <StatusBadge label="Loading tokens" state="warn" />
    {:else if tokenError || formError}
      <StatusBadge label="Action needed" state="err" />
    {:else}
      <StatusBadge label="Ready" state="ok" />
    {/if}
  </div>

  {#if tokenError}
    <AlertBanner variant="err" message={tokenError} />
  {/if}
  {#if tokenSuccess}
    <AlertBanner
      variant="ok"
      message={tokenSuccess}
      onDismiss={() => (tokenSuccess = null)}
    />
  {/if}
  {#if formError}
    <AlertBanner variant="err" message={formError} />
  {/if}
  {#if formSuccess}
    <AlertBanner
      variant="ok"
      message={formSuccess}
      onDismiss={() => (formSuccess = null)}
    />
  {/if}

  <Card
    title="Token management"
    helpSection="sbc_token_management"
    helpContext="server"
  >
    <div class="space-y-5">
      <div class="grid gap-3 md:grid-cols-[1fr_1fr_auto_auto] md:items-end">
        <label class="block">
          <span class="block text-xs font-medium text-text-muted mb-1">
            Display name
            <HelpTip
              fieldKey="display_name"
              sectionKey="sbc_token_management"
              context="server"
            />
          </span>
          <input
            class={inputClass}
            bind:value={createDisplayName}
            placeholder="Start Line"
          />
        </label>
        <label class="block">
          <span class="block text-xs font-medium text-text-muted mb-1">
            Manual token (optional)
            <HelpTip
              fieldKey="manual_token"
              sectionKey="sbc_token_management"
              context="server"
            />
          </span>
          <input
            class={inputClass}
            bind:value={manualToken}
            placeholder="Paste pre-shared token"
            type="password"
            autocomplete="off"
          />
        </label>
        <div class="flex items-center gap-2">
          <button
            type="button"
            class={buttonClass("primary", "md")}
            disabled={tokenBusy != null}
            onclick={() => void createToken(false)}
          >
            {tokenBusy === "generate" ? "Generating…" : "Generate token"}
          </button>
          <HelpTip
            fieldKey="generate_token"
            sectionKey="sbc_token_management"
            context="server"
          />
        </div>
        <div class="flex items-center gap-2">
          <button
            type="button"
            class={buttonClass("secondary", "md")}
            disabled={tokenBusy != null}
            onclick={() => void createToken(true)}
          >
            {tokenBusy === "manual" ? "Adding…" : "Add manual token"}
          </button>
          <HelpTip
            fieldKey="add_manual_token"
            sectionKey="sbc_token_management"
            context="server"
          />
        </div>
      </div>

      {#if createdToken}
        <div
          class="rounded-md border border-status-warn-border bg-status-warn-bg p-4"
        >
          <div class="flex flex-wrap items-center justify-between gap-3">
            <div>
              <p class="text-sm font-semibold text-status-warn m-0">
                One-time token
                <HelpTip
                  fieldKey="one_time_token"
                  sectionKey="sbc_token_management"
                  context="server"
                />
              </p>
              <p class="text-xs text-status-warn mt-1 mb-0">
                This secret is shown only once. Existing token rows show
                metadata only.
              </p>
            </div>
            <button
              type="button"
              class="rounded-md border border-status-warn-border bg-surface-1 px-3 py-1.5 text-xs font-medium text-status-warn"
              onclick={useCreatedToken}>Use in setup form</button
            >
            <HelpTip
              fieldKey="use_in_setup_form"
              sectionKey="sbc_token_management"
              context="server"
            />
          </div>
          <pre
            class="mt-3 overflow-x-auto rounded bg-surface-1 p-3 text-xs text-text-primary"><code
              >{createdToken.token}</code
            ></pre>
        </div>
      {/if}

      <div class="flex items-center justify-between gap-3">
        <p class="text-sm text-text-muted m-0">
          Revoke unused tokens to block first registration. Revoke used tokens
          to block future per-device requests; this does not delete existing
          forwarder status or approval records.
        </p>
        <button
          type="button"
          class={buttonClass("secondary", "xs")}
          disabled={tokensLoading}
          onclick={() => void loadTokens()}>Refresh</button
        >
      </div>

      {#if tokensLoading}
        <p class="text-sm text-text-muted m-0">Loading tokens…</p>
      {:else if forwarderTokens.length === 0}
        <p class="text-sm text-text-muted m-0">No enrollment tokens yet.</p>
      {:else}
        <div class="overflow-x-auto">
          <table class={tableClass}>
            <thead>
              <tr class={tableHeadRowClass}>
                <th class={tableHeaderCellClass()}>Display name</th>
                <th class={tableHeaderCellClass()}>Token ID</th>
                <th class={tableHeaderCellClass()}>Status</th>
                <th class={tableHeaderCellClass()}>Created</th>
                <th class={tableHeaderCellClass()}>Used by endpoint</th>
                <th class={tableHeaderCellClass()}>Used at</th>
                <th class={tableHeaderCellClass()}>Revoked at</th>
                <th class={tableHeaderCellClass()}>Actions</th>
              </tr>
            </thead>
            <tbody>
              {#each forwarderTokens as token (token.token_id)}
                <tr class={tableRowClass}>
                  <td class={tableCellClass(false, "text-text-primary")}
                    >{token.display_name ?? "—"}</td
                  >
                  <td
                    class={tableCellClass(
                      false,
                      "font-mono text-xs text-text-muted",
                    )}>{token.token_id}</td
                  >
                  <td class={tableCellClass()}>
                    <StatusBadge
                      label={token.status}
                      state={tokenState(token.status)}
                    />
                  </td>
                  <td class={tableCellClass(false, "text-text-muted")}
                    >{formatTime(token.created_unix_ms)}</td
                  >
                  <td
                    class={tableCellClass(
                      false,
                      "font-mono text-xs text-text-muted",
                    )}
                    title={token.used_endpoint_id ?? ""}
                  >
                    {endpointShort(token.used_endpoint_id)}
                  </td>
                  <td class={tableCellClass(false, "text-text-muted")}
                    >{formatTime(token.used_unix_ms)}</td
                  >
                  <td class={tableCellClass(false, "text-text-muted")}
                    >{formatTime(token.revoked_unix_ms)}</td
                  >
                  <td class={tableCellClass()}>
                    {#if token.status !== "revoked"}
                      <button
                        type="button"
                        class={buttonClass("danger-soft", "xs")}
                        disabled={tokenBusy != null}
                        onclick={() => void revokeToken(token)}
                      >
                        {tokenBusy === token.token_id ? "Revoking…" : "Revoke"}
                      </button>
                      <HelpTip
                        fieldKey="revoke_token"
                        sectionKey="sbc_token_management"
                        context="server"
                      />
                    {:else}
                      <span class="text-xs text-text-muted">—</span>
                    {/if}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </div>
  </Card>

  <div class="grid gap-6 lg:grid-cols-2">
    <Card
      title="Device identity"
      helpSection="sbc_device_identity"
      helpContext="server"
    >
      <div class="space-y-4">
        <label class="block">
          <span class="block text-xs font-medium text-text-muted mb-1">
            Hostname
            <HelpTip
              fieldKey="hostname"
              sectionKey="sbc_device_identity"
              context="server"
            />
          </span>
          <input class={inputClass} bind:value={form.hostname} />
        </label>
        <label class="block">
          <span class="block text-xs font-medium text-text-muted mb-1">
            SSH admin username
            <HelpTip
              fieldKey="admin_username"
              sectionKey="sbc_device_identity"
              context="server"
            />
          </span>
          <input class={inputClass} bind:value={form.adminUsername} />
        </label>
        <label class="block">
          <span class="block text-xs font-medium text-text-muted mb-1">
            SSH public key
            <HelpTip
              fieldKey="ssh_public_key"
              sectionKey="sbc_device_identity"
              context="server"
            />
          </span>
          <textarea
            class="min-h-24 {inputClass}"
            bind:value={form.sshPublicKey}
            placeholder="ssh-ed25519 …"
          ></textarea>
        </label>
      </div>
    </Card>

    <Card
      title="Network configuration"
      helpSection="sbc_network"
      helpContext="server"
    >
      <div class="space-y-4">
        <label class="block">
          <span class="block text-xs font-medium text-text-muted mb-1">
            Static IPv4/CIDR
            <HelpTip
              fieldKey="static_ipv4_cidr"
              sectionKey="sbc_network"
              context="server"
            />
          </span>
          <input
            class={inputClass}
            bind:value={form.staticIpv4Cidr}
            placeholder="192.168.1.50/24"
          />
        </label>
        <label class="block">
          <span class="block text-xs font-medium text-text-muted mb-1">
            Default gateway
            <HelpTip
              fieldKey="gateway"
              sectionKey="sbc_network"
              context="server"
            />
          </span>
          <input
            class={inputClass}
            bind:value={form.gateway}
            placeholder="192.168.1.1"
          />
        </label>
        <label class="block">
          <span class="block text-xs font-medium text-text-muted mb-1">
            DNS servers
            <HelpTip
              fieldKey="dns_servers"
              sectionKey="sbc_network"
              context="server"
            />
          </span>
          <input
            class={inputClass}
            bind:value={form.dnsServers}
            placeholder="8.8.8.8,8.8.4.4"
          />
        </label>
        <label class="flex items-center gap-2 text-sm text-text-primary">
          <input type="checkbox" bind:checked={form.wifiEnabled} />
          Enable Wi-Fi fallback
          <HelpTip
            fieldKey="wifi_enabled"
            sectionKey="sbc_network"
            context="server"
          />
        </label>
        {#if form.wifiEnabled}
          <div class="grid gap-3 md:grid-cols-3">
            <label class="block md:col-span-2">
              <span class="block text-xs font-medium text-text-muted mb-1">
                Wi-Fi SSID
                <HelpTip
                  fieldKey="wifi_ssid"
                  sectionKey="sbc_network"
                  context="server"
                />
              </span>
              <input class={inputClass} bind:value={form.wifiSsid} />
            </label>
            <label class="block">
              <span class="block text-xs font-medium text-text-muted mb-1">
                Country
                <HelpTip
                  fieldKey="wifi_country"
                  sectionKey="sbc_network"
                  context="server"
                />
              </span>
              <input
                class={inputClass}
                bind:value={form.wifiCountry}
                maxlength="2"
              />
            </label>
          </div>
          <label class="block">
            <span class="block text-xs font-medium text-text-muted mb-1">
              Wi-Fi password
              <HelpTip
                fieldKey="wifi_password"
                sectionKey="sbc_network"
                context="server"
              />
            </span>
            <input
              class={inputClass}
              bind:value={form.wifiPassword}
              type="password"
              autocomplete="off"
              placeholder="Leave blank for open Wi-Fi"
            />
          </label>
        {/if}
      </div>
    </Card>
  </div>

  <div class="grid gap-6 lg:grid-cols-2">
    <Card
      title="Forwarder setup"
      helpSection="sbc_forwarder_setup"
      helpContext="server"
    >
      <div class="space-y-4">
        <label class="block">
          <span class="block text-xs font-medium text-text-muted mb-1">
            Server URL
            <HelpTip
              fieldKey="server_url"
              sectionKey="sbc_forwarder_setup"
              context="server"
            />
          </span>
          <input
            class={inputClass}
            bind:value={form.serverUrl}
            placeholder="https://timer.example.com"
          />
        </label>
        <label class="block">
          <span class="block text-xs font-medium text-text-muted mb-1">
            Auth token
            <HelpTip
              fieldKey="auth_token"
              sectionKey="sbc_forwarder_setup"
              context="server"
            />
          </span>
          <input
            class={inputClass}
            bind:value={form.authToken}
            type="password"
            autocomplete="off"
          />
        </label>
        <label class="block">
          <span class="block text-xs font-medium text-text-muted mb-1">
            Display name
            <HelpTip
              fieldKey="display_name"
              sectionKey="sbc_forwarder_setup"
              context="server"
            />
          </span>
          <input
            class={inputClass}
            bind:value={form.displayName}
            placeholder="Start Line"
          />
        </label>
        <label class="block">
          <span class="block text-xs font-medium text-text-muted mb-1">
            Reader targets
            <HelpTip
              fieldKey="reader_targets"
              sectionKey="sbc_forwarder_setup"
              context="server"
            />
          </span>
          <textarea
            class="min-h-24 {inputClass}"
            bind:value={form.readerTargets}
            placeholder="192.168.1.10:10000"
          ></textarea>
          <span class="mt-1 block text-xs text-text-muted"
            >Separate entries with newlines, commas, or semicolons.</span
          >
        </label>
      </div>
    </Card>

    <Card title="Advanced" helpSection="sbc_advanced" helpContext="server">
      <div class="space-y-4">
        <label class="block">
          <span class="block text-xs font-medium text-text-muted mb-1">
            Status HTTP bind
            <HelpTip
              fieldKey="status_bind"
              sectionKey="sbc_advanced"
              context="server"
            />
          </span>
          <input
            class={inputClass}
            bind:value={form.statusBind}
            placeholder="0.0.0.0:80"
          />
        </label>
        <label class="block">
          <span class="block text-xs font-medium text-text-muted mb-1">
            Setup script URL
            <HelpTip
              fieldKey="setup_script_url"
              sectionKey="sbc_advanced"
              context="server"
            />
          </span>
          <input class={inputClass} bind:value={form.setupScriptUrl} />
        </label>
        <label class="flex items-center gap-2 text-sm text-text-primary">
          <input type="checkbox" bind:checked={form.upsEnabled} />
          Enable UPS HAT support
          <HelpTip
            fieldKey="ups_enabled"
            sectionKey="sbc_advanced"
            context="server"
          />
        </label>
      </div>
    </Card>
  </div>

  <Card
    title="Download actions"
    helpSection="sbc_download_actions"
    helpContext="server"
  >
    <div class="flex flex-wrap items-center gap-3">
      <div class="flex items-center gap-2">
        <button
          type="button"
          class={buttonClass("primary", "md")}
          onclick={downloadUserData}>Download user-data</button
        >
        <HelpTip
          fieldKey="download_user_data"
          sectionKey="sbc_download_actions"
          context="server"
        />
      </div>
      <div class="flex items-center gap-2">
        <button
          type="button"
          class={buttonClass("primary", "md")}
          onclick={downloadNetworkConfig}>Download network-config</button
        >
        <HelpTip
          fieldKey="download_network_config"
          sectionKey="sbc_download_actions"
          context="server"
        />
      </div>
      <div class="flex items-center gap-2">
        <button
          type="button"
          class={buttonClass("secondary", "md")}
          onclick={saveAndNextDevice}>Save &amp; Next Device</button
        >
        <HelpTip
          fieldKey="save_next_device"
          sectionKey="sbc_download_actions"
          context="server"
        />
      </div>
    </div>
    <p class="mt-3 text-xs text-text-muted mb-0">
      Save &amp; Next Device stores non-secret preferences only, clears the auth
      token, and increments host/IP values like rt-fwd-01 and 192.168.1.50/24.
    </p>
  </Card>
</div>
