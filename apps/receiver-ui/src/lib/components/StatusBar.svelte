<script lang="ts">
  import { openUpdateModal, store } from "$lib/store.svelte";

  function computeCounts() {
    const list = store.streams?.streams ?? [];
    let online = 0;
    let degraded = 0;
    let offline = 0;
    let totalReads = 0;
    for (const s of list) {
      if (s.online === true) online++;
      else if (s.online === false) offline++;
      else degraded++;
      if (s.subscribed && s.reads_total !== undefined) {
        totalReads += s.reads_total;
      }
    }
    return { online, degraded, offline, totalReads };
  }

  let c = $derived(computeCounts());
</script>

<div
  class="flex items-center justify-between px-3 h-7 bg-surface-1 border-t border-border shrink-0 text-xs @container"
>
  <div class="flex items-center gap-3">
    {#if c.online > 0}
      <span class="flex items-center gap-1">
        <span class="w-2 h-2 rounded-full bg-status-ok"></span>
        <span class="text-text-muted">{c.online}</span>
        <span class="text-text-muted hidden @[300px]:inline">online</span>
      </span>
    {/if}
    {#if c.degraded > 0}
      <span class="flex items-center gap-1">
        <span class="w-2 h-2 rounded-full bg-status-warn"></span>
        <span class="text-text-muted">{c.degraded}</span>
        <span class="text-text-muted hidden @[300px]:inline">degraded</span>
      </span>
    {/if}
    {#if c.offline > 0}
      <span class="flex items-center gap-1">
        <span class="w-2 h-2 rounded-full bg-status-err"></span>
        <span class="text-text-muted">{c.offline}</span>
        <span class="text-text-muted hidden @[300px]:inline">offline</span>
      </span>
    {/if}
    <span class="font-mono text-text-primary"
      >{c.totalReads.toLocaleString()} reads</span
    >
  </div>

  <div class="flex items-center gap-2 text-text-muted">
    {#if store.status?.receiver_id}
      <span class="font-mono">{store.status.receiver_id}</span>
    {/if}
    {#if store.appVersion}
      <span>v{store.appVersion}</span>
    {/if}
    {#if store.updateState}
      <button
        type="button"
        class="inline-flex h-5 w-5 items-center justify-center rounded-full border border-border bg-surface-0 text-text-primary cursor-pointer hover:bg-surface-2"
        aria-label="Open update details"
        data-testid="update-indicator-btn"
        onclick={() => openUpdateModal()}
      >
        <svg
          viewBox="0 0 16 16"
          class="h-3.5 w-3.5"
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M8 12V4" />
          <path d="M5 7l3-3 3 3" />
        </svg>
      </button>
    {/if}
  </div>
</div>
