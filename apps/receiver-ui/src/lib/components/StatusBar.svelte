<script lang="ts">
  import { store } from "$lib/store.svelte";

  function counts() {
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
</script>

<div
  class="flex items-center justify-between px-3 h-7 bg-surface-1 border-t border-border shrink-0 text-xs @container"
>
  <div class="flex items-center gap-3">
    {#if counts().online > 0}
      <span class="flex items-center gap-1">
        <span class="w-2 h-2 rounded-full bg-status-ok"></span>
        <span class="text-text-muted">{counts().online}</span>
        <span class="text-text-muted hidden @[300px]:inline">online</span>
      </span>
    {/if}
    {#if counts().degraded > 0}
      <span class="flex items-center gap-1">
        <span class="w-2 h-2 rounded-full bg-status-warn"></span>
        <span class="text-text-muted">{counts().degraded}</span>
        <span class="text-text-muted hidden @[300px]:inline">degraded</span>
      </span>
    {/if}
    {#if counts().offline > 0}
      <span class="flex items-center gap-1">
        <span class="w-2 h-2 rounded-full bg-status-err"></span>
        <span class="text-text-muted">{counts().offline}</span>
        <span class="text-text-muted hidden @[300px]:inline">offline</span>
      </span>
    {/if}
    <span class="font-mono text-text-primary"
      >{counts().totalReads.toLocaleString()} reads</span
    >
  </div>

  <div class="flex items-center gap-2 text-text-muted">
    {#if store.status?.receiver_id}
      <span class="font-mono">{store.status.receiver_id}</span>
    {/if}
    {#if store.receiverVersion}
      <span>v{store.receiverVersion}</span>
    {/if}
  </div>
</div>
