<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { AlertBanner, Card, StatusBadge } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";
  import type {
    CreateEnrollmentTokenResponse,
    DeviceRecord,
    EnrollmentTokenRecord,
    StatusResponse,
  } from "$lib/api";

  let status = $state<StatusResponse | null>(null);
  let error = $state<string | null>(null);
  let success = $state<string | null>(null);
  let loading = $state(true);
  let busyEndpoint = $state<string | null>(null);
  let poll: ReturnType<typeof setInterval> | undefined;

  // --- Receiver enrollment tokens ---
  let tokens = $state<EnrollmentTokenRecord[]>([]);
  let receiverTokens = $derived(
    tokens.filter((token) => token.device_kind === "receiver"),
  );
  let tokensLoading = $state(true);
  let tokenBusy = $state<string | null>(null);
  let tokenError = $state<string | null>(null);
  let tokenSuccess = $state<string | null>(null);
  let createdToken = $state<CreateEnrollmentTokenResponse | null>(null);
  let createDisplayName = $state("");
  let manualToken = $state("");
  let copied = $state(false);

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

  function displayName(device: DeviceRecord) {
    return device.display_name?.trim() || "Unnamed device";
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
      success = `Approved ${displayName(approved)} (${approved.endpoint_id}).`;
      await loadStatus();
    } catch (err) {
      error = String(err);
    } finally {
      busyEndpoint = null;
    }
  }

  function endpointShort(endpointId: string | null) {
    if (!endpointId) return "\u2014";
    return endpointId.length > 18
      ? `${endpointId.slice(0, 18)}\u2026`
      : endpointId;
  }

  function formatTime(value: number | null) {
    if (value == null) return "\u2014";
    return new Date(value).toLocaleString();
  }

  function tokenState(tokenStatus: EnrollmentTokenRecord["status"]) {
    if (tokenStatus === "active") return "ok";
    if (tokenStatus === "used") return "warn";
    return "err";
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
    copied = false;
    const trimmedDisplayName = createDisplayName.trim();
    const trimmedManualToken = manualToken.trim();

    if (useManualToken && !trimmedManualToken) {
      tokenBusy = null;
      tokenError = "Manual token is required.";
      return;
    }

    try {
      createdToken = await api.createEnrollmentToken({
        device_kind: "receiver",
        display_name: trimmedDisplayName || undefined,
        token: useManualToken ? trimmedManualToken : undefined,
      });
      manualToken = "";
      tokenSuccess =
        "Receiver token created. Copy the secret now \u2014 it is shown only once.";
      await loadTokens();
    } catch (err) {
      tokenError = String(err);
    } finally {
      tokenBusy = null;
    }
  }

  async function copyToken() {
    if (!createdToken) return;
    try {
      await navigator.clipboard.writeText(createdToken.token);
      copied = true;
      tokenError = null;
      tokenSuccess = "Token copied to clipboard.";
    } catch {
      tokenSuccess = null;
      tokenError = "Copy failed \u2014 select and copy the token manually.";
    }
  }

  async function revokeToken(token: EnrollmentTokenRecord) {
    tokenBusy = token.token_id;
    tokenError = null;
    tokenSuccess = null;
    try {
      await api.revokeEnrollmentToken(token.token_id);
      tokenSuccess = `Revoked ${token.token_id}.`;
      await loadTokens();
    } catch (err) {
      tokenError = String(err);
    } finally {
      tokenBusy = null;
    }
  }

  onMount(() => {
    void loadStatus();
    void loadTokens();
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
        Approve pending forwarders and receivers. Each device shows its assigned
        display name with the endpoint ID underneath for verification.
      </p>
    </div>
    {#if loading}
      <StatusBadge label="Loading" state="warn" />
    {:else if error || tokenError}
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

  <Card title="Receiver enrollment tokens">
    <div class="space-y-5">
      <p class="text-sm text-text-muted m-0">
        Create a one-time enrollment token for a receiver, then enter it (with
        the server URL and receiver ID) in the receiver app's Config tab to
        register a pending receiver for approval below.
      </p>

      <div class="grid gap-3 md:grid-cols-[1fr_1fr_auto_auto] md:items-end">
        <label class="block">
          <span class="block text-xs font-medium text-text-muted mb-1"
            >Display name</span
          >
          <input
            class="w-full rounded-md border border-border bg-surface-1 px-3 py-2 text-sm text-text-primary"
            bind:value={createDisplayName}
            placeholder="Finish Line"
          />
        </label>
        <label class="block">
          <span class="block text-xs font-medium text-text-muted mb-1"
            >Manual token (optional)</span
          >
          <input
            class="w-full rounded-md border border-border bg-surface-1 px-3 py-2 text-sm text-text-primary"
            bind:value={manualToken}
            placeholder="Paste pre-shared token"
            type="password"
            autocomplete="off"
          />
        </label>
        <button
          type="button"
          class="rounded-md border-none bg-accent px-4 py-2 text-sm font-medium text-white hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-50"
          disabled={tokenBusy != null}
          onclick={() => void createToken(false)}
        >
          {tokenBusy === "generate" ? "Generating\u2026" : "Generate token"}
        </button>
        <button
          type="button"
          class="rounded-md border border-border bg-surface-2 px-4 py-2 text-sm font-medium text-text-primary hover:bg-surface-3 disabled:cursor-not-allowed disabled:opacity-50"
          disabled={tokenBusy != null}
          onclick={() => void createToken(true)}
        >
          {tokenBusy === "manual" ? "Adding\u2026" : "Add manual token"}
        </button>
      </div>

      {#if createdToken}
        <div
          class="rounded-md border border-status-warn-border bg-status-warn-bg p-4"
        >
          <div class="flex flex-wrap items-center justify-between gap-3">
            <div>
              <p class="text-sm font-semibold text-status-warn m-0">
                One-time token
              </p>
              <p class="text-xs text-status-warn mt-1 mb-0">
                This secret is shown only once. Existing token rows show
                metadata only.
              </p>
            </div>
            <button
              type="button"
              class="rounded-md border border-status-warn-border bg-surface-1 px-3 py-1.5 text-xs font-medium text-status-warn"
              onclick={() => void copyToken()}
              >{copied ? "Copied" : "Copy token"}</button
            >
          </div>
          <pre
            class="mt-3 overflow-x-auto rounded bg-surface-1 p-3 text-xs text-text-primary"><code
              >{createdToken.token}</code
            ></pre>
        </div>
      {/if}

      <div class="flex items-center justify-between gap-3">
        <p class="text-sm text-text-muted m-0">
          Revoke unused tokens to block first registration. Revoking a used
          token blocks future recovery/re-registration with that voucher; it
          does not deactivate a receiver that already holds a minted per-device
          token.
        </p>
        <button
          type="button"
          class="rounded-md border border-border bg-surface-2 px-3 py-1.5 text-xs font-medium text-text-primary hover:bg-surface-3 disabled:opacity-50"
          disabled={tokensLoading}
          onclick={() => void loadTokens()}>Refresh</button
        >
      </div>

      {#if tokensLoading}
        <p class="text-sm text-text-muted m-0">Loading tokens…</p>
      {:else if receiverTokens.length === 0}
        <p class="text-sm text-text-muted m-0">
          No receiver enrollment tokens yet.
        </p>
      {:else}
        <div class="overflow-x-auto rounded-md border border-border">
          <table class="w-full border-collapse text-left text-sm">
            <thead class="bg-surface-2 text-xs uppercase text-text-muted">
              <tr>
                <th class="px-3 py-2">Display name</th>
                <th class="px-3 py-2">Token ID</th>
                <th class="px-3 py-2">Status</th>
                <th class="px-3 py-2">Created</th>
                <th class="px-3 py-2">Used by endpoint</th>
                <th class="px-3 py-2">Used at</th>
                <th class="px-3 py-2">Revoked at</th>
                <th class="px-3 py-2">Actions</th>
              </tr>
            </thead>
            <tbody>
              {#each receiverTokens as token (token.token_id)}
                <tr class="border-t border-border">
                  <td class="px-3 py-2 text-text-primary"
                    >{token.display_name ?? "\u2014"}</td
                  >
                  <td class="px-3 py-2 font-mono text-xs text-text-muted"
                    >{token.token_id}</td
                  >
                  <td class="px-3 py-2">
                    <StatusBadge
                      label={token.status}
                      state={tokenState(token.status)}
                    />
                  </td>
                  <td class="px-3 py-2 text-text-muted"
                    >{formatTime(token.created_unix_ms)}</td
                  >
                  <td
                    class="px-3 py-2 font-mono text-xs text-text-muted"
                    title={token.used_endpoint_id ?? ""}
                  >
                    {endpointShort(token.used_endpoint_id)}
                  </td>
                  <td class="px-3 py-2 text-text-muted"
                    >{formatTime(token.used_unix_ms)}</td
                  >
                  <td class="px-3 py-2 text-text-muted"
                    >{formatTime(token.revoked_unix_ms)}</td
                  >
                  <td class="px-3 py-2">
                    {#if token.status !== "revoked"}
                      <button
                        type="button"
                        class="rounded-md border border-status-err-border bg-status-err-bg px-3 py-1 text-xs font-medium text-status-err disabled:opacity-50"
                        disabled={tokenBusy != null}
                        onclick={() => void revokeToken(token)}
                      >
                        {tokenBusy === token.token_id
                          ? "Revoking\u2026"
                          : "Revoke"}
                      </button>
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
                {displayName(device)}
              </p>
              <p class="text-xs text-text-muted mt-1 mb-0">
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
              {displayName(device)}
            </p>
            <p class="text-xs text-text-muted mt-1 mb-0">
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
