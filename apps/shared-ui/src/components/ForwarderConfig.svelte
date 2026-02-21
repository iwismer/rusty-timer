<script lang="ts">
  import { onMount } from "svelte";
  import type { ConfigApi } from "../lib/config-types";
  import {
    fromConfig,
    toGeneralPayload,
    toServerPayload,
    toAuthPayload,
    toJournalPayload,
    toUplinkPayload,
    toStatusHttpPayload,
    toControlPayload,
    toReadersPayload,
    validateGeneral,
    validateServer,
    validateAuth,
    validateJournal,
    validateUplink,
    validateStatusHttp,
    validateReaders,
    defaultFallbackPort,
    type ReaderEntry,
    type ForwarderConfigFormState,
  } from "../lib/forwarder-config-form";
  import {
    controlPowerActionsEnabled,
    saveSuccessMessage,
  } from "../lib/forwarder-config-logic";
  import Card from "./Card.svelte";
  import AlertBanner from "./AlertBanner.svelte";
  import StatusBadge from "./StatusBadge.svelte";
  import ConfirmDialog from "./ConfirmDialog.svelte";

  let {
    configApi,
    displayName = undefined,
    isOnline = undefined,
  }: {
    configApi: ConfigApi;
    displayName?: string;
    isOnline?: boolean;
  } = $props();

  let loading = $state(true);
  let loadError: string | null = $state(null);
  let configLoaded = $state(false);
  let restartNeeded = $state(false);
  let sectionMessages: Record<string, { ok: boolean; text: string }> = $state({});
  let savingSection: Record<string, boolean> = $state({});
  let controlActionMessage: { ok: boolean; text: string } | null = $state(null);
  let showAdvanced = $state(false);

  // Form fields
  let generalDisplayName = $state("");
  let serverBaseUrl = $state("");
  let serverForwardersWsPath = $state("");
  let authTokenFile = $state("");
  let journalSqlitePath = $state("");
  let journalPruneWatermarkPct = $state("");
  let uplinkBatchMode = $state("");
  let uplinkBatchFlushMs = $state("");
  let uplinkBatchMaxEvents = $state("");
  let statusHttpBind = $state("");
  let persistedControlAllowPowerActions = $state(false);
  let controlAllowPowerActions = $state(false);
  let readers: ReaderEntry[] = $state([]);
  let powerActionsEnabled = $derived(
    controlPowerActionsEnabled({
      persistedAllowPowerActions: persistedControlAllowPowerActions,
      currentAllowPowerActions: controlAllowPowerActions,
    }),
  );
  let powerActionsDisabledReason = $derived(
    powerActionsEnabled
      ? undefined
      : !controlAllowPowerActions
        ? "Enable and save control power actions first"
        : "Save Control to apply power-action setting",
  );

  type ControlAction = "restartService" | "restartDevice" | "shutdownDevice";
  type ControlActionDetail = {
    title: string;
    message: string;
    confirmLabel: string;
    successMessage: string;
    variant: "warn" | "err";
  };

  const controlActionBusy = $state<Record<ControlAction, boolean>>({
    restartService: false,
    restartDevice: false,
    shutdownDevice: false,
  });

  const controlActionDetails: Record<ControlAction, ControlActionDetail> = {
    restartService: {
      title: "Restart forwarder service?",
      message:
        "Dangerous action: this will restart the forwarder service and briefly interrupt reads and forwarding.",
      confirmLabel: "Restart Service",
      successMessage: "Restart forwarder service initiated.",
      variant: "err" as const,
    },
    restartDevice: {
      title: "Restart forwarder device?",
      message:
        "Dangerous action: this reboots the entire forwarder device and interrupts all timing activity until it comes back.",
      confirmLabel: "Restart Device",
      successMessage: "Restart forwarder device initiated.",
      variant: "err" as const,
    },
    shutdownDevice: {
      title: "Shutdown forwarder device?",
      message:
        "Good practice before unplugging: shut down the forwarder device first to avoid corruption or lost data.",
      confirmLabel: "Shutdown Device",
      successMessage: "Shutdown forwarder device initiated.",
      variant: "warn" as const,
    },
  };

  let pendingControlAction = $state<ControlAction | null>(null);
  let confirmControlActionOpen = $state(false);
  let confirmControlActionBusy = $derived(
    pendingControlAction ? controlActionBusy[pendingControlAction] : false,
  );
  let confirmControlActionTitle = $derived(
    pendingControlAction ? controlActionDetails[pendingControlAction].title : "",
  );
  let confirmControlActionMessage = $derived(
    pendingControlAction ? controlActionDetails[pendingControlAction].message : "",
  );
  let confirmControlActionLabel = $derived(
    pendingControlAction
      ? controlActionDetails[pendingControlAction].confirmLabel
      : "Confirm",
  );
  let confirmControlActionVariant: "warn" | "err" = $derived(
    pendingControlAction
      ? controlActionDetails[pendingControlAction].variant
      : "err",
  );

  onMount(async () => {
    await loadConfig();
  });

  async function loadConfig() {
    loading = true;
    loadError = null;
    try {
      const result = await configApi.getConfig();
      if (!result.ok) {
        throw new Error(result.error ?? "Failed to load config");
      }
      restartNeeded = result.restart_needed;
      applyFormState(fromConfig(result.config));
      configLoaded = true;
    } catch (e) {
      loadError = String(e);
    } finally {
      loading = false;
    }
  }

  function applyFormState(form: ForwarderConfigFormState): void {
    generalDisplayName = form.generalDisplayName;
    serverBaseUrl = form.serverBaseUrl;
    serverForwardersWsPath = form.serverForwardersWsPath;
    authTokenFile = form.authTokenFile;
    journalSqlitePath = form.journalSqlitePath;
    journalPruneWatermarkPct = form.journalPruneWatermarkPct;
    uplinkBatchMode = form.uplinkBatchMode;
    uplinkBatchFlushMs = form.uplinkBatchFlushMs;
    uplinkBatchMaxEvents = form.uplinkBatchMaxEvents;
    statusHttpBind = form.statusHttpBind;
    persistedControlAllowPowerActions = form.controlAllowPowerActions;
    controlAllowPowerActions = form.controlAllowPowerActions;
    readers = form.readers.map((reader) => ({ ...reader }));
  }

  function currentFormState(): ForwarderConfigFormState {
    return {
      generalDisplayName,
      serverBaseUrl,
      serverForwardersWsPath,
      authTokenFile,
      journalSqlitePath,
      journalPruneWatermarkPct,
      uplinkBatchMode,
      uplinkBatchFlushMs,
      uplinkBatchMaxEvents,
      statusHttpBind,
      controlAllowPowerActions,
      readers: readers.map((reader) => ({ ...reader })),
    };
  }

  async function saveSection(
    section: string,
    payload: Record<string, unknown>,
  ): Promise<boolean> {
    sectionMessages[section] = { ok: false, text: "Saving..." };
    savingSection[section] = true;
    try {
      const result = await configApi.saveSection(section, payload);
      if (result.ok) {
        sectionMessages[section] = {
          ok: true,
          text: saveSuccessMessage(result.restart_needed),
        };
        restartNeeded = result.restart_needed;
        return true;
      } else {
        sectionMessages[section] = {
          ok: false,
          text: result.error ?? "Unknown error",
        };
        return false;
      }
    } catch (e) {
      sectionMessages[section] = { ok: false, text: String(e) };
      return false;
    } finally {
      savingSection[section] = false;
    }
  }

  function saveSectionWithValidation(
    section: string,
    validator: ((form: ForwarderConfigFormState) => string | null) | null,
    payloadFn: (form: ForwarderConfigFormState) => Record<string, unknown>,
  ) {
    const form = currentFormState();
    if (validator) {
      const error = validator(form);
      if (error) {
        sectionMessages[section] = { ok: false, text: error };
        return;
      }
    }
    void saveSection(section, payloadFn(form));
  }

  function saveGeneral() {
    saveSectionWithValidation("general", validateGeneral, toGeneralPayload);
  }
  function saveServer() {
    saveSectionWithValidation("server", validateServer, toServerPayload);
  }
  function saveAuth() {
    saveSectionWithValidation("auth", validateAuth, toAuthPayload);
  }
  function saveJournal() {
    saveSectionWithValidation("journal", validateJournal, toJournalPayload);
  }
  function saveUplink() {
    saveSectionWithValidation("uplink", validateUplink, toUplinkPayload);
  }
  function saveStatusHttp() {
    saveSectionWithValidation("status_http", validateStatusHttp, toStatusHttpPayload);
  }
  async function saveControl() {
    const saved = await saveSection("control", toControlPayload(currentFormState()));
    if (saved) {
      persistedControlAllowPowerActions = controlAllowPowerActions;
    }
  }
  function saveReaders() {
    saveSectionWithValidation("readers", validateReaders, toReadersPayload);
  }

  function addReader() {
    readers = [
      ...readers,
      { target: "", enabled: true, local_fallback_port: "" },
    ];
  }
  function removeReader(index: number) {
    readers = readers.filter((_, i) => i !== index);
  }

  function requestControlAction(action: ControlAction): void {
    pendingControlAction = action;
    confirmControlActionOpen = true;
  }

  function cancelControlAction(): void {
    if (confirmControlActionBusy) {
      return;
    }
    confirmControlActionOpen = false;
    pendingControlAction = null;
  }

  async function invokeControlAction(
    action: ControlAction,
  ): Promise<{ ok: boolean; error?: string }> {
    switch (action) {
      case "restartService":
        return configApi.restartService();
      case "restartDevice":
        return configApi.restartDevice();
      case "shutdownDevice":
        return configApi.shutdownDevice();
      default:
        return { ok: false, error: `Unsupported control action: ${action}` };
    }
  }

  async function runPendingControlAction(): Promise<void> {
    if (!pendingControlAction) {
      return;
    }

    const action = pendingControlAction;
    controlActionBusy[action] = true;
    controlActionMessage = null;

    try {
      const result = await invokeControlAction(action);
      if (result.ok) {
        controlActionMessage = {
          ok: true,
          text: controlActionDetails[action].successMessage,
        };
        if (action === "restartService") {
          restartNeeded = false;
        }
      } else {
        controlActionMessage = {
          ok: false,
          text: result.error ?? "Unknown error",
        };
      }
    } catch (e) {
      controlActionMessage = { ok: false, text: String(e) };
    } finally {
      controlActionBusy[action] = false;
      confirmControlActionOpen = false;
      pendingControlAction = null;
    }
  }

  const inputClass =
    "w-full px-2 py-1.5 text-sm rounded-md border border-border bg-surface-0 text-text-primary placeholder:text-text-muted focus:outline-none focus:border-accent";
  const selectClass =
    "w-full px-2 py-1.5 text-sm rounded-md border border-border bg-surface-0 text-text-primary focus:outline-none focus:border-accent";
  const saveBtnClass =
    "mt-2 px-3 py-1.5 text-xs font-medium rounded-md bg-accent text-white border-none cursor-pointer hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed";
  const dangerousActionBtnClass =
    "px-3 py-1.5 text-xs font-medium rounded-md bg-status-err-bg text-status-err border border-status-err-border cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed";
  const cautionActionBtnClass =
    "px-3 py-1.5 text-xs font-medium rounded-md bg-status-warn-bg text-status-warn border border-status-warn-border cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed";
  const hintClass = "text-xs text-text-muted mt-1";
</script>

<div>
  {#if displayName !== undefined}
    <div class="flex items-center gap-3 mb-4">
      <h1 class="text-lg font-bold text-text-primary m-0">
        Configure: {displayName}
      </h1>
      {#if isOnline !== undefined}
        <StatusBadge
          label={isOnline ? "online" : "offline"}
          state={isOnline ? "ok" : "err"}
        />
      {/if}
    </div>
  {/if}

  {#if restartNeeded}
    <div class="mb-4">
      <AlertBanner
        variant="warn"
        message="Restart needed — some changes require a forwarder restart to take effect."
        actionLabel={controlActionBusy.restartService
          ? "Restarting..."
          : "Restart Forwarder Service"}
        actionBusy={controlActionBusy.restartService}
        onAction={() => requestControlAction("restartService")}
      />
    </div>
  {/if}

  {#if controlActionMessage}
    <p
      class="text-sm mb-4 m-0 {controlActionMessage.ok
        ? 'text-status-ok'
        : 'text-status-err'}"
    >
      {controlActionMessage.text}
    </p>
  {/if}

  {#if loading}
    <p class="text-sm text-text-muted">Loading configuration...</p>
  {:else if loadError}
    <div class="mb-4">
      <AlertBanner variant="err" message={loadError} actionLabel="Retry" onAction={loadConfig} />
    </div>
  {:else if configLoaded}
    <div class="space-y-4">
      <!-- Basic Settings -->
      <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
        <!-- General -->
        <Card title="General">
          <label class="block text-sm font-medium text-text-secondary">
            Display Name
            <input type="text" bind:value={generalDisplayName} class="mt-1 {inputClass}" />
            <p class={hintClass}>Optional. Used to identify this forwarder.</p>
          </label>
          <button
            class={saveBtnClass}
            onclick={saveGeneral}
            disabled={savingSection["general"]}
          >
            {savingSection["general"] ? "Saving..." : "Save General"}
          </button>
          {#if sectionMessages["general"]}
            <p
              class="text-xs mt-1 m-0 {sectionMessages['general'].ok
                ? 'text-status-ok'
                : 'text-status-err'}"
            >
              {sectionMessages["general"].text}
            </p>
          {/if}
        </Card>

        <!-- Server -->
        <Card title="Server">
          <div class="space-y-3">
            <label class="block text-sm font-medium text-text-secondary">
              Base URL
              <input type="text" bind:value={serverBaseUrl} class="mt-1 {inputClass}" />
              <p class={hintClass}>HTTP or HTTPS URL of the server. (Automatically converted to WebSocket for communication.)</p>
            </label>
          </div>
          <button
            class={saveBtnClass}
            onclick={saveServer}
            disabled={savingSection["server"]}
          >
            {savingSection["server"] ? "Saving..." : "Save Server"}
          </button>
          {#if sectionMessages["server"]}
            <p
              class="text-xs mt-1 m-0 {sectionMessages['server'].ok
                ? 'text-status-ok'
                : 'text-status-err'}"
            >
              {sectionMessages["server"].text}
            </p>
          {/if}
        </Card>
      </div>

      <Card title="Forwarder Controls">
        <p class={hintClass}>
          These actions affect service/process availability and device power state.
        </p>

        <label class="mt-3 block text-sm font-medium text-text-secondary">
          <span class="inline-flex items-center gap-2">
            <input
              type="checkbox"
              bind:checked={controlAllowPowerActions}
              class="accent-accent"
            />
            Allow restart/shutdown actions for the forwarder device
          </span>
          <p class={hintClass}>
            Required for "Restart Forwarder Device" and "Shutdown Forwarder Device".
          </p>
        </label>
        <button
          class={saveBtnClass}
          onclick={saveControl}
          disabled={savingSection["control"]}
        >
          {savingSection["control"] ? "Saving..." : "Save Control"}
        </button>
        {#if sectionMessages["control"]}
          <p
            class="text-xs mt-1 m-0 {sectionMessages['control'].ok
              ? 'text-status-ok'
              : 'text-status-err'}"
          >
            {sectionMessages["control"].text}
          </p>
        {/if}

        <div class="mt-4 pt-4 border-t border-border">
          <div class="mb-3">
            <p class="text-xs font-semibold uppercase tracking-wide text-status-err m-0">
              Dangerous Actions
            </p>
            <p class="{hintClass} mt-1">
              Confirm before using these actions in production.
            </p>
          </div>
          <div class="flex flex-wrap gap-2">
            <button
              class={dangerousActionBtnClass}
              onclick={() => requestControlAction("restartService")}
              disabled={controlActionBusy.restartService}
            >
              {controlActionBusy.restartService
                ? "Restarting Service..."
                : "Restart Forwarder Service"}
            </button>
            <button
              class={dangerousActionBtnClass}
              onclick={() => requestControlAction("restartDevice")}
              disabled={controlActionBusy.restartDevice || !powerActionsEnabled}
              title={powerActionsDisabledReason}
            >
              {controlActionBusy.restartDevice
                ? "Restarting Device..."
                : "Restart Forwarder Device"}
            </button>
          </div>
          <div class="mt-3">
            <button
              class={cautionActionBtnClass}
              onclick={() => requestControlAction("shutdownDevice")}
              disabled={controlActionBusy.shutdownDevice || !powerActionsEnabled}
              title={powerActionsDisabledReason}
            >
              {controlActionBusy.shutdownDevice
                ? "Shutting Down..."
                : "Shutdown Forwarder Device"}
            </button>
            <p class={hintClass}>
              Good to do before unplugging the forwarder device.
            </p>
          </div>
        </div>
      </Card>

      <!-- Readers -->
      <Card title="Readers">
        <p class={hintClass}>
          IPICO reader devices this forwarder connects to. At least one reader is required.
        </p>
        <div class="overflow-x-auto">
          <table class="w-full text-sm border-collapse">
            <thead>
              <tr class="border-b-2 border-border">
                <th class="text-left py-2 px-2 text-xs font-medium text-text-muted">
                  Target
                  <span class="font-normal block text-text-muted">IP address and port of the reader</span>
                </th>
                <th class="text-left py-2 px-2 text-xs font-medium text-text-muted">Enabled</th>
                <th class="text-left py-2 px-2 text-xs font-medium text-text-muted w-28">
                  Default Port
                </th>
                <th class="text-left py-2 px-2 text-xs font-medium text-text-muted w-28">
                  Port Override
                </th>
                <th class="py-2 px-2"></th>
              </tr>
            </thead>
            <tbody>
              {#each readers as reader, i}
                <tr class="border-b border-border">
                  <td class="py-1.5 px-2">
                    <input
                      type="text"
                      bind:value={reader.target}
                      placeholder="192.168.0.50:10000"
                      aria-label="Reader {i + 1} target"
                      class={inputClass}
                    />
                  </td>
                  <td class="py-1.5 px-2">
                    <input
                      type="checkbox"
                      bind:checked={reader.enabled}
                      aria-label="Reader {i + 1} enabled"
                      class="accent-accent"
                    />
                  </td>
                  <td class="py-1.5 px-2 w-28">
                    <input
                      type="text"
                      disabled
                      value={defaultFallbackPort(reader.target) || "—"}
                      aria-label="Reader {i + 1} default port"
                      class="{inputClass} opacity-50"
                    />
                  </td>
                  <td class="py-1.5 px-2 w-28">
                    <input
                      type="number"
                      bind:value={reader.local_fallback_port}
                      min="1"
                      max="65535"
                      placeholder="None"
                      aria-label="Reader {i + 1} port override"
                      class={inputClass}
                    />
                  </td>
                  <td class="py-1.5 px-2 text-right">
                    <button
                      onclick={() => removeReader(i)}
                      class="px-2 py-1 text-xs rounded-md text-status-err border border-status-err-border bg-status-err-bg cursor-pointer hover:opacity-80"
                    >
                      Remove
                    </button>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
        <div class="flex gap-2 mt-2">
          <button
            onclick={addReader}
            class="px-3 py-1.5 text-xs font-medium rounded-md bg-surface-2 border border-border text-text-secondary cursor-pointer hover:bg-surface-3"
          >
            + Add Reader
          </button>
          <button
            class={saveBtnClass}
            onclick={saveReaders}
            disabled={savingSection["readers"]}
          >
            {savingSection["readers"] ? "Saving..." : "Save Readers"}
          </button>
        </div>
        {#if sectionMessages["readers"]}
          <p
            class="text-xs mt-1 m-0 {sectionMessages['readers'].ok
              ? 'text-status-ok'
              : 'text-status-err'}"
          >
            {sectionMessages["readers"].text}
          </p>
        {/if}
      </Card>

      <!-- Advanced Settings Toggle -->
      <div>
        <button
          onclick={() => (showAdvanced = !showAdvanced)}
          class="text-sm font-medium text-accent hover:underline"
        >
          {showAdvanced ? "▼" : "▶"} Advanced Settings
        </button>
      </div>

      <!-- Advanced Settings -->
      {#if showAdvanced}
        <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
          <!-- Server WS Path -->
          <Card title="Forwarders WebSocket Path">
            <label class="block text-sm font-medium text-text-secondary">
              WebSocket Path
              <input type="text" bind:value={serverForwardersWsPath} class="mt-1 {inputClass}" />
              <p class={hintClass}>Optional. WebSocket endpoint path. Default if unset: auto-detected.</p>
            </label>
            <button
              class={saveBtnClass}
              onclick={saveServer}
              disabled={savingSection["server"]}
            >
              {savingSection["server"] ? "Saving..." : "Save Server"}
            </button>
            {#if sectionMessages["server"]}
              <p
                class="text-xs mt-1 m-0 {sectionMessages['server'].ok
                  ? 'text-status-ok'
                  : 'text-status-err'}"
              >
                {sectionMessages["server"].text}
              </p>
            {/if}
          </Card>

          <!-- Auth -->
          <Card title="Auth">
            <label class="block text-sm font-medium text-text-secondary">
              Token File Path
              <input type="text" bind:value={authTokenFile} class="mt-1 {inputClass}" />
              <p class={hintClass}>Path to file containing authentication token.</p>
            </label>
            <button
              class={saveBtnClass}
              onclick={saveAuth}
              disabled={savingSection["auth"]}
            >
              {savingSection["auth"] ? "Saving..." : "Save Auth"}
            </button>
            {#if sectionMessages["auth"]}
              <p
                class="text-xs mt-1 m-0 {sectionMessages['auth'].ok
                  ? 'text-status-ok'
                  : 'text-status-err'}"
              >
                {sectionMessages["auth"].text}
              </p>
            {/if}
          </Card>
        </div>

        <div class="grid grid-cols-1 md:grid-cols-2 gap-4 items-start">
          <!-- Journal -->
          <Card title="Journal">
            <div class="space-y-3">
              <label class="block text-sm font-medium text-text-secondary">
                SQLite Path
                <input type="text" bind:value={journalSqlitePath} class="mt-1 {inputClass}" />
                <p class={hintClass}>Optional. Path to SQLite journal. Default if unset: in-memory.</p>
              </label>
              <label class="block text-sm font-medium text-text-secondary">
                Prune Watermark %
                <input type="number" bind:value={journalPruneWatermarkPct} min="0" max="100" class="mt-1 {inputClass}" />
                <p class={hintClass}>Trigger journal pruning at this percentage full. Default if unset: 80%.</p>
              </label>
            </div>
            <button
              class={saveBtnClass}
              onclick={saveJournal}
              disabled={savingSection["journal"]}
            >
              {savingSection["journal"] ? "Saving..." : "Save Journal"}
            </button>
            {#if sectionMessages["journal"]}
              <p
                class="text-xs mt-1 m-0 {sectionMessages['journal'].ok
                  ? 'text-status-ok'
                  : 'text-status-err'}"
              >
                {sectionMessages["journal"].text}
              </p>
            {/if}
          </Card>

          <!-- Uplink -->
          <Card title="Uplink">
            <div class="space-y-3">
              <label class="block text-sm font-medium text-text-secondary">
                Batch Mode
                <select bind:value={uplinkBatchMode} class="mt-1 {selectClass}">
                  <option value="">Default (immediate)</option>
                  <option value="immediate">Immediate</option>
                  <option value="batched">Batched</option>
                </select>
                <p class={hintClass}>How to send events to server. Default if unset: immediate.</p>
              </label>
              <label class="block text-sm font-medium text-text-secondary">
                Batch Flush (ms)
                <input type="number" bind:value={uplinkBatchFlushMs} min="0" class="mt-1 {inputClass}" />
                <p class={hintClass}>Max time to wait before sending batch. Default if unset: 100ms.</p>
              </label>
              <label class="block text-sm font-medium text-text-secondary">
                Batch Max Events
                <input type="number" bind:value={uplinkBatchMaxEvents} min="0" class="mt-1 {inputClass}" />
                <p class={hintClass}>Max events per batch. Default if unset: 1000.</p>
              </label>
            </div>
            <button
              class={saveBtnClass}
              onclick={saveUplink}
              disabled={savingSection["uplink"]}
            >
              {savingSection["uplink"] ? "Saving..." : "Save Uplink"}
            </button>
            {#if sectionMessages["uplink"]}
              <p
                class="text-xs mt-1 m-0 {sectionMessages['uplink'].ok
                  ? 'text-status-ok'
                  : 'text-status-err'}"
              >
                {sectionMessages["uplink"].text}
              </p>
            {/if}
          </Card>

          <!-- Status HTTP -->
          <Card title="Status HTTP">
            <label class="block text-sm font-medium text-text-secondary">
              Bind Address
              <input type="text" bind:value={statusHttpBind} class="mt-1 {inputClass}" />
              <p class={hintClass}>IP:port to listen on for status HTTP server. Example: 0.0.0.0:8080. Default if unset: 0.0.0.0:8080.</p>
            </label>
            <button
              class={saveBtnClass}
              onclick={saveStatusHttp}
              disabled={savingSection["status_http"]}
            >
              {savingSection["status_http"] ? "Saving..." : "Save Status HTTP"}
            </button>
            {#if sectionMessages["status_http"]}
              <p
                class="text-xs mt-1 m-0 {sectionMessages['status_http'].ok
                  ? 'text-status-ok'
                  : 'text-status-err'}"
              >
                {sectionMessages["status_http"].text}
              </p>
            {/if}
          </Card>
        </div>
      {/if}
    </div>
  {/if}
</div>

<ConfirmDialog
  open={confirmControlActionOpen}
  title={confirmControlActionTitle}
  message={confirmControlActionMessage}
  confirmLabel={confirmControlActionLabel}
  variant={confirmControlActionVariant}
  busy={confirmControlActionBusy}
  onConfirm={runPendingControlAction}
  onCancel={cancelControlAction}
/>
