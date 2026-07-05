<script lang="ts">
  import { onMount } from "svelte";
  import { HelpTip } from "@rusty-timer/shared-ui";
  import { AdminActions } from "$lib/admin-actions.svelte";
  import { loadAll as globalLoadAll, openHelp } from "$lib/store.svelte";
  import CursorResetSection from "./admin/CursorResetSection.svelte";
  import DangerActionsSection from "./admin/DangerActionsSection.svelte";
  import EarliestEpochSection from "./admin/EarliestEpochSection.svelte";
  import PortOverridesSection from "./admin/PortOverridesSection.svelte";

  // Refresh global store state after every bulk action so other tabs see the
  // changes; Clear Data passes { forceHydrateMode: true }.
  const actions = new AdminActions({
    afterMutate: (opts) => globalLoadAll(opts),
  });

  onMount(() => {
    void actions.loadAll();
  });
</script>

<div class="max-w-[700px] mx-auto px-6 py-6">
  {#if actions.feedback}
    <p
      class="text-sm mb-4 m-0 {actions.feedback.ok
        ? 'text-status-ok'
        : 'text-status-err'}"
      data-testid="admin-feedback"
    >
      {actions.feedback.message}
    </p>
  {/if}

  {#if actions.loading}
    <p class="text-sm text-text-muted">Loading...</p>
  {:else if actions.loadError}
    <p class="text-sm text-status-err">{actions.loadError}</p>
  {:else}
    <div class="space-y-6">
      <!-- Cursor Reset -->
      <section>
        <h3 class="text-sm font-semibold text-text-primary mb-1">
          Cursor Reset
          <HelpTip
            fieldKey="stream_cursor"
            sectionKey="cursor_reset"
            context="receiver-admin"
            onOpenModal={openHelp}
          />
        </h3>
        <CursorResetSection {actions} compact {openHelp} />
      </section>

      <hr class="border-border" />

      <!-- Earliest-Epoch Overrides -->
      <section>
        <h3 class="text-sm font-semibold text-text-primary mb-1">
          Earliest-Epoch Overrides
          <HelpTip
            fieldKey="epoch_override"
            sectionKey="epoch_overrides"
            context="receiver-admin"
            onOpenModal={openHelp}
          />
        </h3>
        <EarliestEpochSection {actions} compact {openHelp} />
      </section>

      <hr class="border-border" />

      <!-- Local Port Overrides -->
      <section>
        <h3 class="text-sm font-semibold text-text-primary mb-1">
          Local Port Overrides
          <HelpTip
            fieldKey="port_override"
            sectionKey="port_overrides"
            context="receiver-admin"
            onOpenModal={openHelp}
          />
        </h3>
        <PortOverridesSection {actions} compact />
      </section>

      <hr class="border-border" />

      <DangerActionsSection {actions} compact showClearData {openHelp} />
    </div>
  {/if}
</div>
