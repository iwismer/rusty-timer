<script lang="ts">
  import { HelpTip } from "@rusty-timer/shared-ui";
  import {
    getOverallHealth,
    openHelp,
    openUpdateModal,
    store,
    type OverallHealth,
  } from "$lib/store.svelte";

  function computeCounts() {
    const list = store.streams?.streams ?? [];
    let totalReads = 0;
    for (const s of list) {
      if (s.subscribed && s.reads_total != null) {
        totalReads += s.reads_total;
      }
    }
    return { totalReads };
  }

  function healthLabel(health: OverallHealth): string {
    if (health === "ok") return "All connected";
    if (health === "err") return "Disconnected";
    return "Some connections degraded";
  }

  function healthDotClass(health: OverallHealth): string {
    if (health === "ok") return "bg-status-ok";
    if (health === "err") return "bg-status-err";
    return "bg-status-warn";
  }

  let c = $derived(computeCounts());
  let overallHealth = $derived(getOverallHealth());
</script>

<div
  class="flex items-center justify-between px-3 h-7 bg-surface-1 border-t border-border shrink-0 text-xs @container"
>
  <div class="flex items-center gap-3">
    <span
      data-testid="overall-health-dot"
      data-health={overallHealth}
      class="h-2.5 w-2.5 rounded-full {healthDotClass(overallHealth)}"
      title={healthLabel(overallHealth)}
      aria-label={healthLabel(overallHealth)}
    ></span>
    <HelpTip
      fieldKey="overall_health"
      sectionKey="status_bar"
      context="receiver"
      onOpenModal={openHelp}
    />
    <span class="font-mono text-text-primary"
      >{c.totalReads.toLocaleString()} reads</span
    >
    <HelpTip
      fieldKey="total_reads"
      sectionKey="status_bar"
      context="receiver"
      onOpenModal={openHelp}
    />
  </div>

  <div class="flex items-center gap-2 text-text-muted">
    {#if store.status?.receiver_id}
      <span class="font-mono">{store.status.receiver_id}</span>
    {/if}
    {#if store.appVersion}
      <span>v{store.appVersion}</span>
      <HelpTip
        fieldKey="identity_version"
        sectionKey="status_bar"
        context="receiver"
        onOpenModal={openHelp}
      />
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
