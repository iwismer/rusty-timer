<script lang="ts">
  import { onMount } from "svelte";
  import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import {
    store,
    setAnnouncerEnabled,
    setAnnouncerMaxListSize,
    importParticipantsFile,
    importChipsFile,
    loadDataStats,
  } from "$lib/store.svelte";
  import { inputClass, btnSecondary } from "$lib/ui-classes";

  onMount(() => {
    void loadDataStats();
  });

  let maxListInput = $state(store.announcerMaxListSize);

  // Keep the input in sync when the store value changes (e.g. profile reload).
  $effect(() => {
    maxListInput = store.announcerMaxListSize;
  });

  function commitMaxList() {
    if (maxListInput !== store.announcerMaxListSize) {
      void setAnnouncerMaxListSize(maxListInput);
    }
  }

  let announcerUrl = $derived(
    store.savedServerUrl
      ? `${store.savedServerUrl.replace(/\/+$/, "")}/announcer`
      : null,
  );

  async function openAnnouncerPage() {
    if (announcerUrl) await openUrl(announcerUrl);
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

<div class="max-w-[560px] mx-auto px-6 py-6">
  <section class="rounded-lg border border-border bg-surface-1 p-4">
    <div class="flex items-center justify-between gap-4">
      <div>
        <p class="text-sm font-medium text-text-primary">
          Announcer publishing
        </p>
        <p class="mt-1 text-xs text-text-muted">
          When on, subscribed streams you select publish finish reads to the
          server announcer board. Choose which streams publish on the Streams
          tab.
        </p>
      </div>
      <label class="inline-flex items-center gap-2 text-xs text-text-muted">
        <input
          data-testid="announcer-enabled-toggle"
          type="checkbox"
          checked={store.announcerEnabled}
          disabled={store.announcerBusy}
          onchange={(e) => setAnnouncerEnabled(e.currentTarget.checked)}
        />
        {store.announcerEnabled ? "On" : "Off"}
      </label>
    </div>

    <div class="mt-4 flex flex-wrap items-end justify-between gap-4">
      <label class="block text-xs font-medium text-text-muted">
        Max finishers shown
        <input
          data-testid="announcer-max-list-input"
          class="{inputClass} mt-1 w-24"
          type="number"
          min="1"
          max="500"
          bind:value={maxListInput}
          disabled={store.announcerMaxListBusy}
          onblur={commitMaxList}
        />
        <span class="mt-1 block text-[11px] text-text-muted">
          Caps how many rows the server announcer feed keeps visible.
        </span>
      </label>

      <button
        data-testid="open-announcer-page-btn"
        type="button"
        class={btnSecondary}
        disabled={!announcerUrl}
        onclick={openAnnouncerPage}
      >
        Open announcer page
      </button>
    </div>
  </section>

  <section class="mt-6 rounded-lg border border-border bg-surface-1 p-4">
    <p class="text-sm font-medium text-text-primary">
      Participant &amp; chip data
    </p>
    <p class="mt-1 text-xs text-text-muted">
      Import a participant file (<code>.ppl</code>) and a chip-assignment file (<code
        >.bibchip</code
      >) so announcer rows show bib and name. Each import replaces all existing
      data of that type.
    </p>

    <div class="mt-4 grid gap-4">
      <div>
        <p class="text-xs font-medium text-text-muted">Participants (.ppl)</p>
        <div class="mt-1 flex items-center gap-3">
          <button
            data-testid="participants-choose-btn"
            type="button"
            class={btnSecondary}
            disabled={store.importBusy}
            onclick={chooseParticipantsFile}
          >
            Choose file…
          </button>
          <span
            data-testid="participants-file-name"
            class="truncate text-xs {store.participantsFilePath
              ? 'text-text-primary'
              : 'text-text-muted'}"
            title={store.participantsFilePath ?? undefined}
          >
            {store.participantsFilePath ?? "No file selected"}
          </span>
        </div>
      </div>

      <div>
        <p class="text-xs font-medium text-text-muted">
          Chip assignments (.bibchip)
        </p>
        <div class="mt-1 flex items-center gap-3">
          <button
            data-testid="chips-choose-btn"
            type="button"
            class={btnSecondary}
            disabled={store.importBusy}
            onclick={chooseChipsFile}
          >
            Choose file…
          </button>
          <span
            data-testid="chips-file-name"
            class="truncate text-xs {store.chipsFilePath
              ? 'text-text-primary'
              : 'text-text-muted'}"
            title={store.chipsFilePath ?? undefined}
          >
            {store.chipsFilePath ?? "No file selected"}
          </span>
        </div>
      </div>
    </div>

    {#if store.dataStats}
      {@const s = store.dataStats}
      <dl
        data-testid="data-stats"
        class="mt-4 grid grid-cols-2 gap-x-4 gap-y-2 rounded-md border border-border bg-surface-0 p-3 text-xs"
      >
        <dt class="text-text-muted">Participants</dt>
        <dd
          data-testid="stat-participants"
          class="text-right text-text-primary"
        >
          {s.participants}
        </dd>
        <dt class="text-text-muted">Chips</dt>
        <dd data-testid="stat-chips" class="text-right text-text-primary">
          {s.chips}
        </dd>
        <dt class="text-text-muted">Matched (bib + chip)</dt>
        <dd data-testid="stat-matched" class="text-right text-text-primary">
          {s.matched_participants}
        </dd>
        <dt class="text-text-muted">Participants missing chips</dt>
        <dd
          data-testid="stat-missing"
          class="text-right {s.participants_without_chips > 0
            ? 'text-status-warn'
            : 'text-text-primary'}"
        >
          {s.participants_without_chips}
        </dd>
        {#if s.chips - s.resolvable_chips > 0}
          <dt class="text-text-muted">Chips with no participant</dt>
          <dd
            data-testid="stat-unmatched-chips"
            class="text-right text-status-warn"
          >
            {s.chips - s.resolvable_chips}
          </dd>
        {/if}
      </dl>
    {/if}

    {#if store.importMessage}
      <p data-testid="import-message" class="mt-3 text-xs text-status-ok">
        {store.importMessage}
      </p>
    {/if}
    {#if store.importError}
      <p data-testid="import-error" class="mt-3 text-xs text-status-err">
        {store.importError}
      </p>
    {/if}
  </section>
</div>
