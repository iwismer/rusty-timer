<script lang="ts">
  import { HelpTip } from "@rusty-timer/shared-ui";
  import {
    store,
    getModeDirty,
    markModeEdited,
    applyMode,
    setModeDraft,
    openHelp,
  } from "$lib/store.svelte";
  import type { ReceiverMode } from "$lib/api";
  import { inputClass, btnPrimary } from "$lib/ui-classes";
</script>

<div class="max-w-[500px] mx-auto px-6 py-6">
  <div class="grid gap-4">
    <label class="block text-xs font-medium text-text-muted">
      Mode
      <HelpTip
        fieldKey="mode"
        sectionKey="receiver_mode"
        context="receiver"
        onOpenModal={openHelp}
      />
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
      </select>
    </label>

    <p class="text-xs text-text-muted m-0">
      {#if store.modeDraft === "live"}
        Live mode includes all available streams automatically. Use the
        per-stream "From epoch" control in the Streams tab to skip older epochs.
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
