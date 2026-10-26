<script lang="ts">
  import { onMount } from "svelte";
  import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import {
    store,
    closeRaceDayModal,
    openRaceDayWizard,
    getOrderedStreams,
    getStreamRole,
    setStreamRole,
    moveStreamOrder,
    getStreamDriftMs,
    bulkSyncClocks,
    completeRaceDaySetup,
    resetAnnouncer,
    toggleSubscription,
    setStreamAnnouncerPublish,
    setAnnouncerEnabled,
    setAnnouncerMaxListSize,
    loadDbfConfig,
    loadRdImportConfig,
    saveDbfConfig,
    saveRdImportConfig,
    loadDataStats,
    importParticipantsFile,
    importChipsFile,
    streamPrimaryLabel,
    streamForwarderLabel,
    streamIdentity,
    type StreamRole,
  } from "$lib/store.svelte";
  import type * as api from "$lib/api";

  let step = $state(1);
  let forceShowManualImport = $state(false);
  let syncingClocks = $state(false);
  let finishing = $state(false);
  let finishError = $state<string | null>(null);

  // Step 7 Epoch Names state
  let sfStartEpoch = $state("Mass Start");
  let sfFinishEpoch = $state("Finish Reads");
  let startEpoch = $state("Start Reads");
  let finishEpoch = $state("Finish Reads");
  let announcerEpoch = $state("Announcer Feed");
  let clearLocalReads = $state(true);
  let resetAnnouncerFeed = $state(true);

  const stepTitles = [
    "Select Forwarders & Readers",
    "Assign Reader Roles & Course Order",
    "Race Director & DBF Frequencies",
    "Fallback Participant / Chip Import",
    "Announcer Screen Setup",
    "Reader Clock Synchronization",
    "Epoch Names & Reset",
  ];

  let orderedStreams = $derived(getOrderedStreams());
  let subscribedStreams = $derived(orderedStreams.filter((s) => s.subscribed));

  let hasParticipants = $derived((store.dataStats?.participants ?? 0) > 0);

  // Group membership checks for Step 7
  let hasStartFinish = $derived(
    subscribedStreams.some((s) => getStreamRole(s) === "start_finish"),
  );
  let hasStart = $derived(
    subscribedStreams.some((s) => getStreamRole(s) === "start"),
  );
  let hasFinish = $derived(
    subscribedStreams.some((s) => getStreamRole(s) === "finish"),
  );
  let hasAnnouncer = $derived(
    subscribedStreams.some((s) => getStreamRole(s) === "announcer"),
  );

  let startFinishStreams = $derived(
    subscribedStreams.filter((s) => getStreamRole(s) === "start_finish"),
  );
  let startStreams = $derived(
    subscribedStreams.filter((s) => getStreamRole(s) === "start"),
  );
  let finishStreams = $derived(
    subscribedStreams.filter((s) => getStreamRole(s) === "finish"),
  );
  let announcerStreams = $derived(
    subscribedStreams.filter((s) => getStreamRole(s) === "announcer"),
  );

  let announcerUrl = $derived(
    store.savedServerUrl
      ? `${store.savedServerUrl.replace(/\/+$/, "")}/announcer`
      : null,
  );

  let hasAnnouncerPublish = $derived(
    subscribedStreams.some(
      (s) => getStreamRole(s) === "announcer" || s.announcer_publish,
    ),
  );

  let userToggledAnnouncer = $state(false);
  let announcerScreenEnabled = $state(false);
  let announcerMaxList = $state(20);

  $effect(() => {
    if (!store.raceDayModalOpen) {
      userToggledAnnouncer = false;
      announcerScreenEnabled = false;
      announcerMaxList = 20;
      finishError = null;
      return;
    }
    if (!userToggledAnnouncer) {
      announcerScreenEnabled = hasAnnouncerPublish || store.announcerEnabled;
      announcerMaxList = store.announcerMaxListSize || 20;
    }
  });

  async function handleOpenAnnouncer() {
    if (announcerUrl) {
      try {
        await openUrl(announcerUrl);
      } catch (e) {
        console.warn("Failed to open announcer URL:", e);
      }
    }
  }

  function getForwarderReaders(fwd: api.ForwarderConnectionStatus) {
    const knownStreams = (store.streams?.streams ?? []).filter(
      (s) => s.forwarder_endpoint_id === fwd.endpoint_id,
    );
    const result: Array<{
      stream_id: string;
      stream: api.StreamEntry | null;
      subscribed: boolean;
      connected: boolean;
      local_port: number | null;
    }> = [];

    const seenStreamIds = new Set<string>();

    for (const stream of knownStreams) {
      seenStreamIds.add(stream.stream_id);
      if (stream.reader_ip) seenStreamIds.add(stream.reader_ip);
      const liveReader = fwd.readers?.find(
        (r) =>
          r.stream_id === stream.stream_id ||
          (stream.reader_ip != null && r.stream_id === stream.reader_ip),
      );
      if (liveReader) seenStreamIds.add(liveReader.stream_id);
      result.push({
        stream_id: stream.stream_id,
        stream,
        subscribed: stream.subscribed,
        connected:
          liveReader?.connected ??
          stream.reader_connected ??
          stream.online ??
          false,
        local_port: stream.local_port,
      });
    }

    for (const reader of fwd.readers ?? []) {
      if (!seenStreamIds.has(reader.stream_id)) {
        seenStreamIds.add(reader.stream_id);
        result.push({
          stream_id: reader.stream_id,
          stream: null,
          subscribed: false,
          connected: reader.connected,
          local_port: null,
        });
      }
    }

    return result;
  }

  onMount(() => {
    loadDbfConfig();
    loadRdImportConfig();
    loadDataStats();
  });

  function handlePrev() {
    if (step === 5 && hasParticipants && !forceShowManualImport) {
      step = 3;
    } else if (step > 1) {
      step -= 1;
    }
  }

  async function handleNext() {
    if (step === 3) {
      // Save Step 3 configs
      await saveDbfConfig();
      await saveRdImportConfig();

      if (hasParticipants && !forceShowManualImport) {
        step = 5; // Skip step 4 if participants detected
      } else {
        step = 4;
      }
    } else if (step === 5) {
      await setAnnouncerEnabled(announcerScreenEnabled);
      await setAnnouncerMaxListSize(announcerMaxList);
      step = 6;
    } else if (step < 7) {
      step += 1;
    } else if (step === 7) {
      await handleFinish();
    }
  }

  async function handleFinish() {
    finishing = true;
    finishError = null;
    try {
      const res = await completeRaceDaySetup({
        startFinishStartEpoch: hasStartFinish ? sfStartEpoch : undefined,
        startFinishFinishEpoch: hasStartFinish ? sfFinishEpoch : undefined,
        startEpoch: hasStart ? startEpoch : undefined,
        finishEpoch: hasFinish ? finishEpoch : undefined,
        announcerEpoch: hasAnnouncer ? announcerEpoch : undefined,
        clearLocalReads,
        resetDbfFile: clearLocalReads && Boolean(store.editDbfEnabled),
        resetAnnouncerFeed,
      });
      if (!res.success && res.error) {
        finishError = res.error;
      }
    } catch (e) {
      finishError = e instanceof Error ? e.message : String(e);
    } finally {
      finishing = false;
    }
  }

  async function handleSyncAllClocks() {
    syncingClocks = true;
    try {
      await bulkSyncClocks(subscribedStreams);
    } finally {
      syncingClocks = false;
    }
  }

  function singlePath(path: string | string[] | null): string | null {
    return typeof path === "string" ? path : null;
  }

  async function chooseParticipantsFile() {
    const path = singlePath(
      await openFileDialog({
        multiple: false,
        filters: [
          { name: "Participant files", extensions: ["ppl", "csv", "txt"] },
        ],
      }),
    );
    if (path) await importParticipantsFile(path);
  }

  async function chooseChipsFile() {
    const path = singlePath(
      await openFileDialog({
        multiple: false,
        filters: [
          {
            name: "Chip assignment files",
            extensions: ["bibchip", "csv", "txt"],
          },
        ],
      }),
    );
    if (path) await importChipsFile(path);
  }
</script>

{#if store.raceDayModalOpen}
  <div
    class="fixed inset-0 bg-black/60 backdrop-blur-sm z-50 flex items-center justify-center p-4"
    role="dialog"
    aria-modal="true"
    aria-labelledby="race-day-modal-title"
  >
    {#if store.raceDayModalMode === "prompt"}
      <!-- Startup Prompt -->
      <div
        class="bg-surface-0 border border-border rounded-xl shadow-2xl max-w-md w-full overflow-hidden"
      >
        <div class="p-6">
          <div class="flex items-center gap-3">
            <div
              class="w-10 h-10 rounded-full bg-accent/10 border border-accent/30 flex items-center justify-center text-accent shrink-0"
            >
              <svg
                class="w-5 h-5"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                <path
                  d="M4 15s1-1 4-1 5 2 8 2 4-1 4-1V3s-1 1-4 1-5-2-8-2-4 1-4 1z"
                />
                <line x1="4" y1="22" x2="4" y2="15" />
              </svg>
            </div>
            <div>
              <h2
                id="race-day-modal-title"
                class="text-base font-bold text-text-primary"
              >
                Race Day Setup
              </h2>
              <p class="text-xs text-text-muted mt-0.5">
                Rusty Timer Race Day Configuration
              </p>
            </div>
          </div>
          <p class="mt-4 text-xs text-text-secondary leading-relaxed">
            Is this a <strong>new race day</strong>? Starting a new race day
            will walk you through forwarders and readers, roles, course flow
            order, clock synchronization, and clearing local test reads.
          </p>
        </div>
        <div
          class="bg-surface-1 border-t border-border px-6 py-4 flex flex-col gap-2"
        >
          <button
            type="button"
            data-testid="race-day-new-btn"
            class="w-full py-2 px-3 rounded-md bg-accent hover:bg-accent-hover text-white font-medium text-xs flex items-center justify-center gap-1.5 cursor-pointer shadow-sm transition-colors"
            onclick={() => openRaceDayWizard()}
          >
            <span>Yes, Set Up New Race Day</span>
            <svg
              class="w-3.5 h-3.5"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <polyline points="9 18 15 12 9 6" />
            </svg>
          </button>
          <button
            type="button"
            data-testid="race-day-resume-btn"
            class="w-full py-1.5 px-3 rounded-md bg-surface-0 hover:bg-surface-2 text-text-secondary font-medium text-xs border border-border cursor-pointer transition-colors"
            onclick={() => closeRaceDayModal()}
          >
            No, Resume Previous Setup & State
          </button>
        </div>
      </div>
    {:else}
      <!-- 6-Step Guided Wizard -->
      <div
        class="bg-surface-0 border border-border rounded-xl shadow-2xl max-w-2xl w-full flex flex-col max-h-[90vh] overflow-hidden"
      >
        <!-- Wizard Header -->
        <div class="p-4 border-b border-border bg-surface-1 shrink-0">
          <div class="flex items-center justify-between">
            <div>
              <span
                class="text-[11px] font-semibold uppercase tracking-wider text-accent"
              >
                Step {step} of {stepTitles.length}
              </span>
              <h2
                id="race-day-modal-title"
                class="text-sm font-bold text-text-primary"
              >
                {stepTitles[step - 1]}
              </h2>
            </div>
            <button
              type="button"
              class="text-text-muted hover:text-text-primary p-1 rounded cursor-pointer"
              aria-label="Close setup modal"
              onclick={() => closeRaceDayModal()}
            >
              <svg
                class="w-4 h-4"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
              >
                <line x1="18" y1="6" x2="6" y2="18" />
                <line x1="6" y1="6" x2="18" y2="18" />
              </svg>
            </button>
          </div>

          <!-- Progress Pills -->
          <div class="grid grid-cols-7 gap-1.5 mt-2.5">
            {#each [1, 2, 3, 4, 5, 6, 7] as s}
              <div
                class="h-1 rounded-full transition-colors {s === step
                  ? 'bg-accent'
                  : s < step
                    ? 'bg-accent/60'
                    : 'bg-surface-3'}"
              ></div>
            {/each}
          </div>
        </div>

        <!-- Wizard Content Body -->
        <div class="flex-1 overflow-y-auto p-5 text-text-primary text-xs">
          <!-- Step 1: Forwarders & Readers -->
          {#if step === 1}
            <div class="space-y-4">
              <p class="text-text-secondary leading-relaxed">
                Select the forwarders connected to your receiver today, and
                configure which reader streams to subscribe to.
              </p>

              {#if store.connections?.forwarders && store.connections.forwarders.length > 0}
                <div class="space-y-3">
                  {#each store.connections.forwarders as fwd}
                    {@const readers = getForwarderReaders(fwd)}
                    <div
                      class="rounded-lg border border-border bg-surface-1 p-3 space-y-2.5"
                    >
                      <div
                        class="flex items-center justify-between border-b border-border pb-2"
                      >
                        <div class="flex items-center gap-2">
                          <span
                            class="font-semibold text-text-primary font-mono"
                          >
                            {fwd.display_name || fwd.endpoint_id}
                          </span>
                          <span class="text-[11px] text-text-muted font-mono">
                            ({fwd.endpoint_id.slice(0, 10)})
                          </span>
                        </div>
                        <span
                          class="px-2 py-0.5 rounded text-[10px] uppercase font-semibold {fwd.state ===
                          'subscribed'
                            ? 'bg-status-ok-bg text-status-ok border border-status-ok-border'
                            : 'bg-surface-2 text-text-muted border border-border'}"
                        >
                          {fwd.state}
                        </span>
                      </div>

                      <!-- Readers under forwarder -->
                      <div class="space-y-1.5 pl-2">
                        {#if readers.length === 0}
                          {#if fwd.pending || fwd.state === "disconnected" || fwd.state === "unavailable"}
                            <div
                              class="flex items-center gap-2 py-1.5 text-[11px] text-text-muted"
                            >
                              <span
                                class="inline-block w-2.5 h-2.5 rounded-full border-2 border-accent border-t-transparent animate-spin"
                              ></span>
                              <span
                                >Connecting to forwarder and discovering
                                readers…</span
                              >
                            </div>
                          {:else}
                            <p class="text-[11px] text-text-muted italic py-1">
                              No reader targets configured on this forwarder.
                            </p>
                          {/if}
                        {:else}
                          {#each readers as item (item.stream_id)}
                            <div
                              class="flex items-center justify-between p-2 rounded bg-surface-0 border border-border"
                            >
                              <label
                                class="flex items-center gap-2 cursor-pointer font-mono text-text-primary select-none"
                              >
                                <input
                                  type="checkbox"
                                  checked={item.subscribed}
                                  onchange={() => {
                                    if (item.stream) {
                                      void toggleSubscription(item.stream);
                                    } else {
                                      void toggleSubscription({
                                        forwarder_endpoint_id: fwd.endpoint_id,
                                        stream_id: item.stream_id,
                                        subscribed: false,
                                      });
                                    }
                                  }}
                                  class="accent-accent cursor-pointer"
                                />
                                <span>{item.stream_id}</span>
                                {#if item.local_port}
                                  <span class="text-text-muted text-[11px]">
                                    (port {item.local_port})
                                  </span>
                                {/if}
                              </label>
                              <span
                                class="text-[10px] font-semibold uppercase px-1.5 py-0.5 rounded {item.connected
                                  ? 'text-status-ok bg-status-ok-bg border border-status-ok-border'
                                  : 'text-text-muted bg-surface-2 border border-border'}"
                              >
                                {item.connected ? "connected" : "offline"}
                              </span>
                            </div>
                          {/each}
                        {/if}
                      </div>
                    </div>
                  {/each}
                </div>
              {:else if store.streams && store.streams.streams.length > 0}
                <!-- Fallback to streams list -->
                <div class="space-y-2">
                  {#each store.streams.streams as stream}
                    <div
                      class="flex items-center justify-between p-2.5 rounded bg-surface-1 border border-border"
                    >
                      <label
                        class="flex items-center gap-2 cursor-pointer font-mono"
                      >
                        <input
                          type="checkbox"
                          checked={stream.subscribed}
                          onchange={() => toggleSubscription(stream)}
                          class="accent-accent"
                        />
                        <span>{streamPrimaryLabel(stream)}</span>
                        <span class="text-text-muted text-[11px]">
                          ({streamForwarderLabel(stream)})
                        </span>
                      </label>
                      <span
                        class="text-[10px] uppercase font-semibold {stream.subscribed
                          ? 'text-status-ok'
                          : 'text-text-muted'}"
                      >
                        {stream.subscribed ? "Subscribed" : "Available"}
                      </span>
                    </div>
                  {/each}
                </div>
              {:else}
                <div
                  class="p-4 rounded-lg bg-surface-1 border border-border text-center text-text-muted space-y-1"
                >
                  <p class="font-medium">
                    No forwarders or streams detected yet.
                  </p>
                  <p class="text-[11px]">
                    Ensure your timing boxes are powered on and connected via
                    local network or P2P iroh.
                  </p>
                </div>
              {/if}
            </div>

            <!-- Step 2: Assign Reader Roles & Course Order -->
          {:else if step === 2}
            <div class="space-y-4">
              <p class="text-text-secondary leading-relaxed">
                Assign each reader's role and arrange them in course flow order
                (e.g. Announcer &rarr; Finish &rarr; Backup).
              </p>

              {#if subscribedStreams.length === 0}
                <div
                  class="p-4 rounded-lg bg-surface-1 border border-border text-center text-text-muted"
                >
                  No subscribed streams selected. Please go back to Step 1 and
                  subscribe to at least one reader stream.
                </div>
              {:else}
                <div class="space-y-3">
                  {#each subscribedStreams as stream, index (streamIdentity(stream))}
                    {@const role = getStreamRole(stream)}
                    <div
                      class="p-3 rounded-lg border border-border bg-surface-1 space-y-2.5"
                    >
                      <div class="flex items-center justify-between">
                        <div class="flex items-center gap-2">
                          <span
                            class="font-mono font-bold text-text-primary text-xs"
                          >
                            {streamPrimaryLabel(stream)}
                          </span>
                          <span class="text-[11px] text-text-muted font-mono">
                            ({streamForwarderLabel(stream)})
                          </span>
                        </div>

                        <!-- Order Adjuster -->
                        <div class="flex items-center gap-1.5">
                          <span class="text-text-muted text-[11px]"
                            >Course Order:</span
                          >
                          <div
                            class="flex items-center gap-1 bg-surface-0 px-2 py-0.5 rounded border border-border font-mono text-xs font-bold"
                          >
                            <span>#{index + 1}</span>
                            <button
                              type="button"
                              class="p-0.5 leading-none hover:text-accent disabled:opacity-30 cursor-pointer disabled:cursor-not-allowed"
                              disabled={index === 0}
                              title="Move earlier in course order"
                              aria-label="Move earlier in course order"
                              onclick={() =>
                                moveStreamOrder(stream, -1, subscribedStreams)}
                            >
                              &uarr;
                            </button>
                            <button
                              type="button"
                              class="p-0.5 leading-none hover:text-accent disabled:opacity-30 cursor-pointer disabled:cursor-not-allowed"
                              disabled={index === subscribedStreams.length - 1}
                              title="Move later in course order"
                              aria-label="Move later in course order"
                              onclick={() =>
                                moveStreamOrder(stream, 1, subscribedStreams)}
                            >
                              &darr;
                            </button>
                          </div>
                        </div>
                      </div>

                      <!-- 4 Roles Radio Group -->
                      <div
                        class="grid grid-cols-2 sm:grid-cols-4 gap-2 text-xs"
                      >
                        <!-- Start / Finish -->
                        <label
                          class="p-2 rounded border cursor-pointer flex flex-col gap-0.5 transition-colors {role ===
                          'start_finish'
                            ? 'border-amber-500/50 bg-amber-500/10 font-semibold text-amber-800 dark:text-amber-300'
                            : 'border-border bg-surface-0 text-text-primary hover:bg-surface-2'}"
                        >
                          <div class="flex items-center gap-1.5">
                            <input
                              type="radio"
                              name={`role-${streamIdentity(stream)}`}
                              checked={role === "start_finish"}
                              onchange={() =>
                                setStreamRole(stream, "start_finish")}
                              class="accent-amber-600"
                            />
                            <span>Start / Finish</span>
                          </div>
                          <span class="text-[10px] text-text-muted font-normal">
                            Cutover ready
                          </span>
                        </label>

                        <!-- Start -->
                        <label
                          class="p-2 rounded border cursor-pointer flex flex-col gap-0.5 transition-colors {role ===
                          'start'
                            ? 'border-blue-500/50 bg-blue-500/10 font-semibold text-blue-800 dark:text-blue-300'
                            : 'border-border bg-surface-0 text-text-primary hover:bg-surface-2'}"
                        >
                          <div class="flex items-center gap-1.5">
                            <input
                              type="radio"
                              name={`role-${streamIdentity(stream)}`}
                              checked={role === "start"}
                              onchange={() => setStreamRole(stream, "start")}
                              class="accent-blue-600"
                            />
                            <span>Start</span>
                          </div>
                          <span class="text-[10px] text-text-muted font-normal">
                            Manual changes
                          </span>
                        </label>

                        <!-- Finish -->
                        <label
                          class="p-2 rounded border cursor-pointer flex flex-col gap-0.5 transition-colors {role ===
                          'finish'
                            ? 'border-emerald-500/50 bg-emerald-500/10 font-semibold text-emerald-800 dark:text-emerald-300'
                            : 'border-border bg-surface-0 text-text-primary hover:bg-surface-2'}"
                        >
                          <div class="flex items-center gap-1.5">
                            <input
                              type="radio"
                              name={`role-${streamIdentity(stream)}`}
                              checked={role === "finish"}
                              onchange={() => setStreamRole(stream, "finish")}
                              class="accent-emerald-600"
                            />
                            <span>Finish</span>
                          </div>
                          <span class="text-[10px] text-text-muted font-normal">
                            Manual changes
                          </span>
                        </label>

                        <!-- Announcer -->
                        <label
                          class="p-2 rounded border cursor-pointer flex flex-col gap-0.5 transition-colors {role ===
                          'announcer'
                            ? 'border-purple-500/50 bg-purple-500/10 font-semibold text-purple-800 dark:text-purple-300'
                            : 'border-border bg-surface-0 text-text-primary hover:bg-surface-2'}"
                        >
                          <div class="flex items-center gap-1.5">
                            <input
                              type="radio"
                              name={`role-${streamIdentity(stream)}`}
                              checked={role === "announcer"}
                              onchange={() =>
                                setStreamRole(stream, "announcer")}
                              class="accent-purple-600"
                            />
                            <span>Announcer</span>
                          </div>
                          <span class="text-[10px] text-text-muted font-normal">
                            Excluded from DBF
                          </span>
                        </label>
                      </div>

                      {#if role === "start_finish" || role === "finish"}
                        <div class="pt-1 pl-1">
                          <label
                            class="inline-flex items-center gap-2 cursor-pointer text-xs text-text-secondary select-none"
                          >
                            <input
                              type="checkbox"
                              data-testid={`role-announcer-toggle-${streamIdentity(stream)}`}
                              checked={stream.announcer_publish ?? false}
                              onchange={(e) => {
                                void setStreamAnnouncerPublish(
                                  stream,
                                  e.currentTarget.checked,
                                );
                              }}
                              class="accent-accent cursor-pointer"
                            />
                            <span>Also publish reads to Announcer screen</span>
                          </label>
                        </div>
                      {/if}
                    </div>
                  {/each}
                </div>
              {/if}
            </div>

            <!-- Step 3: Race Director & DBF Frequencies -->
          {:else if step === 3}
            <div class="space-y-4">
              <p class="text-text-secondary leading-relaxed">
                Configure integration with The Race Director. Reads are
                formatted for Ipico Direct.
              </p>

              <div
                class="p-3.5 rounded-lg border border-border bg-surface-1 space-y-3.5"
              >
                <label class="block font-medium text-text-secondary">
                  Race Director Folder:
                  <input
                    type="text"
                    bind:value={store.editRdImportDir}
                    class="mt-1 w-full px-2.5 py-1.5 rounded-md border border-border bg-surface-0 text-xs font-mono text-text-primary"
                    placeholder="C:\Winrace\Files"
                  />
                </label>

                <!-- DBF Writing Controls -->
                <div
                  class="p-2.5 rounded bg-surface-0 border border-border space-y-2"
                >
                  <label
                    class="flex items-center gap-2 cursor-pointer font-medium text-text-primary select-none"
                  >
                    <input
                      type="checkbox"
                      bind:checked={store.editDbfEnabled}
                      class="accent-accent cursor-pointer"
                    />
                    <span>Write reads to IPICO.DBF for Race Director</span>
                  </label>
                  <div
                    class="flex items-center gap-2 pl-5 text-text-muted text-[11px]"
                  >
                    <span>Write interval:</span>
                    <input
                      type="number"
                      min="0.25"
                      max="5.0"
                      step="0.25"
                      value={store.editDbfFlushIntervalMs / 1000}
                      oninput={(e) => {
                        const val = parseFloat(e.currentTarget.value);
                        if (!isNaN(val) && val > 0) {
                          store.editDbfFlushIntervalMs = Math.round(val * 1000);
                        }
                      }}
                      class="w-16 px-1.5 py-0.5 rounded border border-border bg-surface-1 font-mono text-xs text-text-primary"
                    />
                    <span>seconds (0.25&ndash;5.0s)</span>
                  </div>
                </div>

                <!-- RD Polling Controls -->
                <div
                  class="p-2.5 rounded bg-surface-0 border border-border space-y-2"
                >
                  <label
                    class="flex items-center gap-2 cursor-pointer font-medium text-text-primary select-none"
                  >
                    <input
                      type="checkbox"
                      bind:checked={store.editRdImportEnabled}
                      class="accent-accent cursor-pointer"
                    />
                    <span
                      >Poll Race Director for participant &amp; chip data</span
                    >
                  </label>
                  <div
                    class="flex items-center gap-2 pl-5 text-text-muted text-[11px]"
                  >
                    <span>Poll interval:</span>
                    <input
                      type="number"
                      min="1"
                      max="60"
                      step="1"
                      bind:value={store.editRdImportIntervalSecs}
                      class="w-16 px-1.5 py-0.5 rounded border border-border bg-surface-1 font-mono text-xs text-text-primary"
                    />
                    <span>seconds (1&ndash;60s)</span>
                  </div>
                </div>

                <!-- Live Participant Detection Banner -->
                {#if hasParticipants}
                  <div
                    class="p-2.5 rounded border border-status-ok-border bg-status-ok-bg text-status-ok flex items-center justify-between"
                  >
                    <span>
                      Found <strong
                        >{store.dataStats?.participants} participants</strong
                      >
                      and
                      <strong>{store.dataStats?.chips} chip mappings</strong>
                      in {store.editRdImportDir}
                    </span>
                    <button
                      type="button"
                      class="text-[11px] underline font-medium hover:opacity-80 cursor-pointer ml-2 shrink-0"
                      onclick={() => {
                        forceShowManualImport = true;
                        step = 4;
                      }}
                    >
                      Review manual file import
                    </button>
                  </div>
                {/if}
              </div>
            </div>

            <!-- Step 4: Fallback Participant / Chip Import -->
          {:else if step === 4}
            <div class="space-y-4">
              {#if hasParticipants}
                <div
                  class="p-2.5 rounded border border-amber-500/30 bg-amber-500/10 text-amber-800 dark:text-amber-300 text-xs"
                >
                  Race Director polling is active with {store.dataStats
                    ?.participants} participants. Use this only if you need to manually
                  import files.
                </div>
              {:else}
                <p class="text-text-secondary leading-relaxed">
                  If Race Director polling is not active, select participant and
                  chip assignment files manually.
                </p>
              {/if}

              <div
                class="p-3.5 rounded-lg border border-border bg-surface-1 space-y-3"
              >
                <div class="flex items-center justify-between">
                  <div>
                    <div class="font-medium text-text-primary">
                      Participants (.ppl / .csv)
                    </div>
                    <div class="text-[11px] text-text-muted">
                      {store.dataStats?.participants ?? 0} participants currently
                      loaded
                    </div>
                  </div>
                  <div>
                    <button
                      type="button"
                      class="px-2.5 py-1 rounded border border-border bg-surface-0 hover:bg-surface-2 text-text-primary cursor-pointer text-xs font-medium"
                      onclick={chooseParticipantsFile}
                    >
                      Choose file…
                    </button>
                  </div>
                </div>

                <div
                  class="flex items-center justify-between border-t border-border pt-3"
                >
                  <div>
                    <div class="font-medium text-text-primary">
                      Chip Assignments (.bibchip / .csv)
                    </div>
                    <div class="text-[11px] text-text-muted">
                      {store.dataStats?.chips ?? 0} chip mappings currently active
                    </div>
                  </div>
                  <div>
                    <button
                      type="button"
                      class="px-2.5 py-1 rounded border border-border bg-surface-0 hover:bg-surface-2 text-text-primary cursor-pointer text-xs font-medium"
                      onclick={chooseChipsFile}
                    >
                      Choose file…
                    </button>
                  </div>
                </div>

                {#if store.importMessage}
                  <div
                    class="p-2 rounded bg-surface-0 border border-border text-[11px] text-status-ok"
                  >
                    {store.importMessage}
                  </div>
                {:else if store.importError}
                  <div
                    class="p-2 rounded bg-surface-0 border border-status-err-border text-[11px] text-status-err"
                  >
                    {store.importError}
                  </div>
                {/if}
              </div>
            </div>

            <!-- Step 5: Announcer Screen Setup -->
          {:else if step === 5}
            <div class="space-y-4">
              <p class="text-text-secondary leading-relaxed">
                Configure the Announcer Screen board to show real-time athlete
                names and finish times to your race announcer.
              </p>

              <div class="space-y-3">
                <label
                  class="flex items-center gap-3 cursor-pointer text-xs font-semibold text-text-primary select-none p-3.5 rounded-lg border border-border bg-surface-1"
                >
                  <input
                    type="checkbox"
                    data-testid="wizard-announcer-enable-toggle"
                    bind:checked={announcerScreenEnabled}
                    onchange={() => {
                      userToggledAnnouncer = true;
                    }}
                    class="accent-accent cursor-pointer w-4 h-4"
                  />
                  <div>
                    <div>Enable Announcer Screen</div>
                    <div class="text-[11px] text-text-muted font-normal">
                      Pushes live participant reads to the Announcer Board for
                      finish line announcers.
                    </div>
                  </div>
                </label>

                <div
                  class="p-3.5 rounded-lg border border-border bg-surface-1 space-y-2"
                >
                  <label class="block text-xs font-medium text-text-secondary">
                    Max finishers shown on board:
                    <input
                      type="number"
                      min="1"
                      max="500"
                      data-testid="wizard-announcer-max-list"
                      bind:value={announcerMaxList}
                      class="mt-1 w-24 px-2 py-1 rounded border border-border bg-surface-0 font-mono text-xs text-text-primary"
                    />
                  </label>
                  <p class="text-[11px] text-text-muted">
                    Controls the maximum number of recent finishers displayed
                    simultaneously on the announcer screen.
                  </p>
                </div>

                {#if announcerUrl}
                  <div
                    class="p-3.5 rounded-lg border border-border bg-surface-1 flex items-center justify-between gap-3"
                  >
                    <div>
                      <div class="text-xs font-medium text-text-primary">
                        Announcer Board Display
                      </div>
                      <div
                        class="text-[11px] text-text-muted font-mono truncate max-w-xs"
                      >
                        {announcerUrl}
                      </div>
                    </div>
                    <button
                      type="button"
                      class="px-2.5 py-1 rounded border border-border bg-surface-0 hover:bg-surface-2 text-xs font-medium text-text-primary cursor-pointer flex items-center gap-1.5 shrink-0"
                      onclick={handleOpenAnnouncer}
                    >
                      <svg
                        class="w-3.5 h-3.5"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                      >
                        <path
                          d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"
                        />
                        <polyline points="15 3 21 3 21 9" />
                        <line x1="10" y1="14" x2="21" y2="3" />
                      </svg>
                      <span>Open Announcer Board</span>
                    </button>
                  </div>
                {/if}

                <!-- Reset Online Announcer Feed -->
                <div class="pt-2 border-t border-border space-y-3">
                  <label
                    class="flex items-center gap-3 cursor-pointer text-xs font-semibold text-text-primary select-none p-3.5 rounded-lg border border-border bg-surface-1"
                  >
                    <input
                      type="checkbox"
                      data-testid="wizard-announcer-reset-toggle"
                      bind:checked={resetAnnouncerFeed}
                      class="accent-accent cursor-pointer w-4 h-4"
                    />
                    <div>
                      <div>Reset Online Announcer Feed for Race Day</div>
                      <div class="text-[11px] text-text-muted font-normal">
                        Clears previous finishers from the online announcer
                        screen so it starts clean for the day.
                      </div>
                    </div>
                  </label>

                  <div
                    class="p-3.5 rounded-lg border border-border bg-surface-1 flex items-center justify-between gap-3"
                  >
                    <div>
                      <div class="text-xs font-medium text-text-primary">
                        Reset Announcer Feed Now
                      </div>
                      <div class="text-[11px] text-text-muted">
                        Immediately wipe all current finishers on the announcer
                        board.
                      </div>
                    </div>
                    <button
                      type="button"
                      data-testid="wizard-announcer-reset-now-btn"
                      class="px-2.5 py-1 rounded border border-border bg-surface-0 hover:bg-surface-2 text-xs font-medium text-text-primary cursor-pointer disabled:opacity-50"
                      disabled={store.announcerResetBusy}
                      onclick={() => resetAnnouncer()}
                    >
                      {store.announcerResetBusy
                        ? "Resetting..."
                        : "Reset Feed Now"}
                    </button>
                  </div>

                  {#if store.announcerResetMessage}
                    <div
                      data-testid="wizard-announcer-reset-success"
                      class="text-xs text-status-ok font-medium px-1"
                    >
                      {store.announcerResetMessage}
                    </div>
                  {/if}
                  {#if store.announcerResetError}
                    <div
                      data-testid="wizard-announcer-reset-error"
                      class="text-xs text-status-err font-medium px-1"
                    >
                      {store.announcerResetError}
                    </div>
                  {/if}
                </div>
              </div>
            </div>

            <!-- Step 6: Reader Clock Synchronization -->
          {:else if step === 6}
            <div class="space-y-4">
              <p class="text-text-secondary leading-relaxed">
                Verify reader clock offsets against your timing computer and
                synchronize before the race starts.
              </p>

              {#if subscribedStreams.length === 0}
                <div
                  class="p-4 rounded-lg bg-surface-1 border border-border text-center text-text-muted"
                >
                  No subscribed streams selected.
                </div>
              {:else}
                <div class="space-y-2">
                  {#each subscribedStreams as stream (streamIdentity(stream))}
                    {@const driftMs = getStreamDriftMs(stream)}
                    <div
                      class="p-2.5 rounded-lg border border-border bg-surface-1 flex items-center justify-between"
                    >
                      <div class="flex items-center gap-2">
                        <span class="font-mono font-medium text-text-primary">
                          {streamPrimaryLabel(stream)}
                        </span>
                        <span class="text-text-muted text-[11px] font-mono">
                          ({streamForwarderLabel(stream)})
                        </span>
                      </div>
                      <div>
                        {#if driftMs != null}
                          {@const abs = Math.abs(driftMs)}
                          <span
                            class="font-mono px-2 py-0.5 rounded text-xs font-medium border {abs <
                            250
                              ? 'bg-status-ok-bg text-status-ok border-status-ok-border'
                              : abs < 1000
                                ? 'bg-amber-500/10 text-amber-700 dark:text-amber-400 border-amber-500/30'
                                : 'bg-status-err-bg text-status-err border-status-err-border'}"
                          >
                            {driftMs > 0 ? `+${driftMs}ms` : `${driftMs}ms`}
                          </span>
                        {:else}
                          <span class="text-text-muted font-mono text-[11px]">
                            &ndash;
                          </span>
                        {/if}
                      </div>
                    </div>
                  {/each}
                </div>

                <div class="pt-2 flex justify-end">
                  <button
                    type="button"
                    data-testid="sync-all-clocks-btn"
                    class="px-3 py-1.5 rounded-md bg-accent hover:bg-accent-hover text-white font-medium text-xs flex items-center gap-1.5 cursor-pointer shadow-sm disabled:opacity-50 transition-colors"
                    disabled={syncingClocks}
                    onclick={handleSyncAllClocks}
                  >
                    <svg
                      class="w-3.5 h-3.5 {syncingClocks ? 'animate-spin' : ''}"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="2"
                    >
                      <circle cx="12" cy="12" r="10" />
                      <polyline points="12 6 12 12 16 14" />
                    </svg>
                    <span
                      >{syncingClocks
                        ? "Syncing Clocks…"
                        : "Sync All Reader Clocks"}</span
                    >
                  </button>
                </div>
              {/if}
            </div>

            <!-- Step 7: Epoch Names by Reader Group & Reset -->
          {:else if step === 7}
            <div class="space-y-4">
              <p class="text-text-secondary leading-relaxed">
                Configure initial epoch names for each reader group. For <strong
                  >Start / Finish</strong
                > readers, set both the initial start epoch and the cutover finish
                epoch.
              </p>

              {#if finishError}
                <div
                  class="p-3 rounded-lg border border-status-err-border bg-status-err-bg text-status-err text-xs space-y-2"
                >
                  <div class="font-semibold">Race day setup warning/error:</div>
                  <div>{finishError}</div>
                  <div class="flex justify-end gap-2 pt-1">
                    <button
                      type="button"
                      class="px-2.5 py-1 rounded border border-status-err text-xs font-medium cursor-pointer hover:bg-surface-0"
                      onclick={closeRaceDayModal}
                    >
                      Close Modal Anyway
                    </button>
                  </div>
                </div>
              {/if}

              <div
                class="p-3.5 rounded-lg border border-border bg-surface-1 space-y-3.5"
              >
                <!-- Start/Finish Group -->
                {#if hasStartFinish}
                  <div
                    class="p-2.5 rounded bg-surface-0 border border-amber-500/30 space-y-2"
                  >
                    <span
                      class="font-semibold text-amber-800 dark:text-amber-400"
                    >
                      Start / Finish Readers ({startFinishStreams
                        .map(streamPrimaryLabel)
                        .join(", ")})
                    </span>
                    <div class="grid grid-cols-1 sm:grid-cols-2 gap-2 pt-1">
                      <label class="block text-text-secondary font-medium">
                        Initial Start Epoch:
                        <input
                          type="text"
                          data-testid="sf-start-epoch-input"
                          bind:value={sfStartEpoch}
                          class="mt-0.5 w-full px-2 py-1 rounded border border-border bg-surface-1 font-mono text-xs text-text-primary"
                        />
                      </label>
                      <label class="block text-text-secondary font-medium">
                        Cutover Finish Epoch:
                        <input
                          type="text"
                          data-testid="sf-finish-epoch-input"
                          bind:value={sfFinishEpoch}
                          class="mt-0.5 w-full px-2 py-1 rounded border border-border bg-surface-1 font-mono text-xs text-text-primary"
                        />
                      </label>
                    </div>
                  </div>
                {/if}

                <!-- Start Group -->
                {#if hasStart}
                  <div
                    class="p-2.5 rounded bg-surface-0 border border-blue-500/30 space-y-2"
                  >
                    <span
                      class="font-semibold text-blue-800 dark:text-blue-400"
                    >
                      Start Readers ({startStreams
                        .map(streamPrimaryLabel)
                        .join(", ")})
                    </span>
                    <label class="block text-text-secondary font-medium pt-1">
                      Start Epoch Name:
                      <input
                        type="text"
                        data-testid="start-epoch-input"
                        bind:value={startEpoch}
                        class="mt-0.5 w-full px-2 py-1 rounded border border-border bg-surface-1 font-mono text-xs text-text-primary"
                      />
                    </label>
                  </div>
                {/if}

                <!-- Finish Group -->
                {#if hasFinish}
                  <div
                    class="p-2.5 rounded bg-surface-0 border border-emerald-500/30 space-y-2"
                  >
                    <span
                      class="font-semibold text-emerald-800 dark:text-emerald-400"
                    >
                      Finish Readers ({finishStreams
                        .map(streamPrimaryLabel)
                        .join(", ")})
                    </span>
                    <label class="block text-text-secondary font-medium pt-1">
                      Finish Epoch Name:
                      <input
                        type="text"
                        data-testid="finish-epoch-input"
                        bind:value={finishEpoch}
                        class="mt-0.5 w-full px-2 py-1 rounded border border-border bg-surface-1 font-mono text-xs text-text-primary"
                      />
                    </label>
                  </div>
                {/if}

                <!-- Announcer Group -->
                {#if hasAnnouncer}
                  <div
                    class="p-2.5 rounded bg-surface-0 border border-purple-500/30 space-y-2"
                  >
                    <span
                      class="font-semibold text-purple-800 dark:text-purple-400"
                    >
                      Announcer Readers ({announcerStreams
                        .map(streamPrimaryLabel)
                        .join(", ")})
                    </span>
                    <label class="block text-text-secondary font-medium pt-1">
                      Announcer Epoch Name:
                      <input
                        type="text"
                        data-testid="announcer-epoch-input"
                        bind:value={announcerEpoch}
                        class="mt-0.5 w-full px-2 py-1 rounded border border-border bg-surface-1 font-mono text-xs text-text-primary"
                      />
                    </label>
                  </div>
                {/if}

                <!-- Clear local test reads / reset DBF -->
                <label
                  class="flex items-center gap-2 cursor-pointer p-2.5 rounded border border-red-500/30 bg-red-500/5 text-red-600 dark:text-red-400 font-medium select-none"
                >
                  <input
                    type="checkbox"
                    data-testid="wizard-clear-reads-toggle"
                    bind:checked={clearLocalReads}
                    class="accent-red-600 cursor-pointer"
                  />
                  <span data-testid="wizard-clear-reads-label">
                    {#if store.editDbfEnabled}
                      Clear local test reads and reset DBF
                    {:else}
                      Clear local test reads
                    {/if}
                  </span>
                </label>

                {#if resetAnnouncerFeed}
                  <div
                    data-testid="wizard-summary-announcer-reset"
                    class="text-xs text-text-secondary flex items-center gap-2 px-1"
                  >
                    <span class="w-1.5 h-1.5 rounded-full bg-status-ok"></span>
                    Online Announcer feed will be reset
                  </div>
                {/if}
              </div>
            </div>
          {/if}
        </div>

        <!-- Wizard Navigation Footer -->
        <div
          class="p-3 border-t border-border bg-surface-1 flex items-center justify-between shrink-0"
        >
          <button
            type="button"
            data-testid="wizard-prev-btn"
            class="px-3 py-1.5 rounded-md border border-border bg-surface-0 hover:bg-surface-2 text-xs font-medium text-text-secondary cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
            disabled={step === 1 || finishing}
            onclick={handlePrev}
          >
            &larr; Back
          </button>

          <button
            type="button"
            data-testid="wizard-next-btn"
            class="px-3.5 py-1.5 rounded-md bg-accent hover:bg-accent-hover text-white text-xs font-semibold cursor-pointer disabled:opacity-50 shadow-sm transition-colors"
            disabled={finishing}
            onclick={handleNext}
          >
            {#if finishing}
              Finishing Setup…
            {:else if step === 7}
              Finish &amp; Start Race Day
            {:else}
              Next &rarr;
            {/if}
          </button>
        </div>
      </div>
    {/if}
  </div>
{/if}
