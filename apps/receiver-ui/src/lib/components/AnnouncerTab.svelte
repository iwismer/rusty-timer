<script lang="ts">
  import { openUrl } from "@tauri-apps/plugin-opener";
  import {
    store,
    setAnnouncerEnabled,
    setAnnouncerMaxListSize,
    importParticipantsText,
    importChipsText,
  } from "$lib/store.svelte";
  import { inputClass, btnSecondary } from "$lib/ui-classes";

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

  // Legacy Race Director `.ppl`/`.bibchip` files are frequently Windows-1252
  // (accented names). `File.text()` assumes UTF-8 and would mangle them, so
  // decode strict UTF-8 first and fall back to Windows-1252 on invalid bytes.
  async function decodeFile(file: File): Promise<string> {
    const buf = await file.arrayBuffer();
    try {
      return new TextDecoder("utf-8", { fatal: true }).decode(buf);
    } catch {
      return new TextDecoder("windows-1252").decode(buf);
    }
  }

  async function onParticipantsFile(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;
    const text = await decodeFile(file);
    await importParticipantsText(text);
    input.value = "";
  }

  async function onChipsFile(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;
    const text = await decodeFile(file);
    await importChipsText(text);
    input.value = "";
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
      <label class="block text-xs font-medium text-text-muted">
        Participants (.ppl)
        <input
          data-testid="participants-file-input"
          class="{inputClass} mt-1"
          type="file"
          accept=".ppl,.csv,text/plain"
          disabled={store.importBusy}
          onchange={onParticipantsFile}
        />
      </label>

      <label class="block text-xs font-medium text-text-muted">
        Chip assignments (.bibchip)
        <input
          data-testid="chips-file-input"
          class="{inputClass} mt-1"
          type="file"
          accept=".bibchip,.csv,text/plain"
          disabled={store.importBusy}
          onchange={onChipsFile}
        />
      </label>
    </div>

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
