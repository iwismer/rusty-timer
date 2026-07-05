<script lang="ts">
  import { onMount } from "svelte";
  import { Card } from "@rusty-timer/shared-ui";
  import { AdminActions } from "$lib/admin-actions.svelte";
  import CursorResetSection from "$lib/components/admin/CursorResetSection.svelte";
  import DangerActionsSection from "$lib/components/admin/DangerActionsSection.svelte";
  import EarliestEpochSection from "$lib/components/admin/EarliestEpochSection.svelte";
  import PortOverridesSection from "$lib/components/admin/PortOverridesSection.svelte";

  const actions = new AdminActions();

  onMount(() => {
    void actions.loadAll();
  });
</script>

<svelte:head>
  <title>Receiver Admin · Rusty Timer</title>
</svelte:head>

<main class="max-w-[960px] mx-auto px-6 py-6">
  <div class="flex items-center justify-between mb-6">
    <h1 class="text-xl font-bold text-text-primary m-0">Receiver Admin</h1>
  </div>

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
      <Card
        title="Cursor Reset"
        borderStatus="warn"
        helpSection="cursor_reset"
        helpContext="receiver-admin"
      >
        <CursorResetSection {actions} />
      </Card>

      <!-- Earliest-Epoch Overrides -->
      <Card
        title="Earliest-Epoch Overrides"
        borderStatus="warn"
        helpSection="epoch_overrides"
        helpContext="receiver-admin"
      >
        <EarliestEpochSection {actions} />
      </Card>

      <!-- Local Port Overrides -->
      <Card
        title="Local Port Overrides"
        helpSection="port_overrides"
        helpContext="receiver-admin"
      >
        <PortOverridesSection {actions} />
      </Card>

      <DangerActionsSection {actions} />
    </div>
  {/if}
</main>
