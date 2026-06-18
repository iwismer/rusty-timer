<script lang="ts">
  import { HelpTip } from "@rusty-timer/shared-ui";
  import {
    store,
    getModeDirty,
    markModeEdited,
    applyMode,
    setModeDraft,
  } from "$lib/store.svelte";
  import type { ReceiverMode } from "$lib/api";
  import { inputClass, btnPrimary } from "$lib/ui-classes";
</script>

<div class="max-w-[500px] mx-auto px-6 py-6">
  <div class="grid gap-4">
    <label class="block text-xs font-medium text-text-muted">
      Mode
      <HelpTip fieldKey="mode" sectionKey="receiver_mode" context="receiver" />
      <select
        data-testid="mode-select"
        class="{inputClass} mt-1"
        value={store.modeDraft}
        onchange={(e) => {
          setModeDraft(e.currentTarget.value as ReceiverMode["mode"]);
          markModeEdited();
        }}
        disabled={store.modeBusy}
      >
        <option value="live">Live</option>
        <option value="targeted_replay">Targeted Replay</option>
      </select>
    </label>

    <p class="text-xs text-text-muted m-0">
      {#if store.modeDraft === "live"}
        Live mode includes all available streams automatically and supports
        earliest-epoch overrides.
      {:else}
        Targeted Replay uses per-stream epoch controls in the Streams tab.
      {/if}
    </p>
  </div>

  <div class="mt-4 pt-4 border-t border-border">
    <button
      data-testid="save-mode-btn"
      class={btnPrimary}
      onclick={() => void applyMode()}
      disabled={!getModeDirty() || store.modeBusy}
    >
      {store.modeBusy ? "Applying\u2026" : "Apply Mode"}
    </button>
  </div>
</div>
