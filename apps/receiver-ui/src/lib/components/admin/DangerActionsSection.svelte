<script lang="ts">
  import { Card, HelpTip, buttonClass } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";
  import { type AdminActions } from "$lib/admin-actions.svelte";

  const btnWarn = buttonClass("warn", "xs");
  const btnDanger = buttonClass("danger", "sm");
  const btnDangerConfirm = buttonClass("danger-solid", "sm");
  const btnNeutral = buttonClass("secondary", "xs");

  let {
    actions,
    compact = false,
    showClearData = false,
    openHelp = undefined,
  }: {
    actions: AdminActions;
    /** Embedded-tab density; the comfortable variant is the /admin route. */
    compact?: boolean;
    /** Clear Data only exists in the embedded (compact) variant. */
    showClearData?: boolean;
    /** Store-driven help modal opener (embedded tab only). */
    openHelp?: (fieldKey: string) => void;
  } = $props();

  let confirmingClearData = $state(false);
  let confirmingFactoryReset = $state(false);

  async function handleClearData() {
    confirmingClearData = false;
    await actions.bulkAction(
      () => api.clearData(),
      "Clear data",
      "clear-data",
      {
        forceHydrateMode: true,
      },
    );
  }

  async function handleFactoryReset() {
    confirmingFactoryReset = false;
    await actions.bulkAction(
      () => api.factoryReset(),
      "Factory reset",
      "factory-reset",
    );
  }
</script>

{#if compact}
  <!-- Purge Subscriptions -->
  <section>
    <h3 class="text-sm font-semibold text-text-primary mb-1">
      Purge Subscriptions
    </h3>
    <p class="text-xs text-text-muted m-0 mb-3">
      Remove all stream subscriptions.
    </p>
    <button
      onclick={() =>
        actions.bulkAction(
          () => api.purgeSubscriptions(),
          "Purge subscriptions",
          "purge-subs",
        )}
      disabled={actions.inFlightAction === "purge-subs" ||
        actions.subscriptions.length === 0}
      class={btnWarn}
    >
      {actions.inFlightAction === "purge-subs"
        ? "Purging..."
        : "Purge All Subscriptions"}
    </button>
    <HelpTip
      fieldKey="purge_all_subscriptions"
      sectionKey="purge_subscriptions"
      context="receiver-admin"
      onOpenModal={openHelp}
    />
  </section>

  <hr class="border-border" />

  <!-- Reset Profile -->
  <section>
    <h3 class="text-sm font-semibold text-text-primary mb-1">Reset Profile</h3>
    <p class="text-xs text-text-muted m-0 mb-3">
      Clear server URL, token, and receiver ID back to defaults.
    </p>
    <button
      onclick={() =>
        actions.bulkAction(
          () => api.resetProfile(),
          "Reset profile",
          "reset-profile",
        )}
      disabled={actions.inFlightAction === "reset-profile"}
      class={btnWarn}
    >
      {actions.inFlightAction === "reset-profile"
        ? "Resetting..."
        : "Reset Profile to Defaults"}
    </button>
    <HelpTip
      fieldKey="reset_profile_action"
      sectionKey="reset_profile"
      context="receiver-admin"
      onOpenModal={openHelp}
    />
  </section>

  {#if showClearData}
    <hr class="border-border" />

    <!-- Clear Data -->
    <section>
      <h3 class="text-sm font-semibold text-status-err mb-1">
        Clear Data
        <HelpTip
          fieldKey="clear_data_action"
          sectionKey="clear_data"
          context="receiver-admin"
          onOpenModal={openHelp}
        />
      </h3>
      <p class="text-xs text-text-muted m-0 mb-3">
        Clear local subscriptions, cursors, mode, and DBF config. Keeps the
        server URL, token, and receiver ID.
      </p>
      {#if confirmingClearData}
        <div class="flex items-center gap-3">
          <span class="text-sm text-status-err font-medium">Are you sure?</span>
          <button
            onclick={handleClearData}
            disabled={actions.inFlightAction === "clear-data"}
            class={btnDangerConfirm}
          >
            {actions.inFlightAction === "clear-data"
              ? "Clearing..."
              : "Yes, Clear Data"}
          </button>
          <HelpTip
            fieldKey="clear_data_action"
            sectionKey="clear_data"
            context="receiver-admin"
            onOpenModal={openHelp}
          />
          <button
            onclick={() => (confirmingClearData = false)}
            class={btnNeutral}
          >
            Cancel
          </button>
        </div>
      {:else}
        <button onclick={() => (confirmingClearData = true)} class={btnDanger}>
          Clear Data...
        </button>
        <HelpTip
          fieldKey="clear_data_action"
          sectionKey="clear_data"
          context="receiver-admin"
          onOpenModal={openHelp}
        />
      {/if}
    </section>
  {/if}

  <hr class="border-border" />

  <!-- Factory Reset -->
  <section>
    <h3 class="text-sm font-semibold text-status-err mb-1">
      Factory Reset
      <HelpTip
        fieldKey="factory_reset_action"
        sectionKey="factory_reset"
        context="receiver-admin"
        onOpenModal={openHelp}
      />
    </h3>
    <p class="text-xs text-text-muted m-0 mb-3">
      Clear <strong>all</strong> local data. This cannot be undone.
    </p>
    {#if confirmingFactoryReset}
      <div class="flex items-center gap-3">
        <span class="text-sm text-status-err font-medium">Are you sure?</span>
        <button
          onclick={handleFactoryReset}
          disabled={actions.inFlightAction === "factory-reset"}
          class={btnDangerConfirm}
        >
          {actions.inFlightAction === "factory-reset"
            ? "Resetting..."
            : "Yes, Factory Reset"}
        </button>
        <HelpTip
          fieldKey="factory_reset_action"
          sectionKey="factory_reset"
          context="receiver-admin"
          onOpenModal={openHelp}
        />
        <button
          onclick={() => (confirmingFactoryReset = false)}
          class={btnNeutral}
        >
          Cancel
        </button>
      </div>
    {:else}
      <button onclick={() => (confirmingFactoryReset = true)} class={btnDanger}>
        Factory Reset...
      </button>
      <HelpTip
        fieldKey="factory_reset_action"
        sectionKey="factory_reset"
        context="receiver-admin"
        onOpenModal={openHelp}
      />
    {/if}
  </section>
{:else}
  <!-- Purge Subscriptions -->
  <Card
    title="Purge Subscriptions"
    borderStatus="warn"
    helpSection="purge_subscriptions"
    helpContext="receiver-admin"
  >
    <p class="text-sm text-text-muted m-0 mb-4">
      Remove all stream subscriptions. The receiver will have no streams until
      new ones are added.
    </p>
    <button
      onclick={() =>
        actions.bulkAction(
          () => api.purgeSubscriptions(),
          "Purge subscriptions",
          "purge-subs",
        )}
      disabled={actions.inFlightAction === "purge-subs" ||
        actions.subscriptions.length === 0}
      class="px-3 py-1.5 text-xs font-medium rounded-md text-status-warn border border-status-warn-border bg-status-warn-bg cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
    >
      {actions.inFlightAction === "purge-subs"
        ? "Purging..."
        : "Purge All Subscriptions"}
    </button>
  </Card>

  <!-- Reset Profile -->
  <Card
    title="Reset Profile"
    borderStatus="warn"
    helpSection="reset_profile"
    helpContext="receiver-admin"
  >
    <p class="text-sm text-text-muted m-0 mb-4">
      Clear server URL, token, and receiver ID back to defaults. The receiver
      will need to be reconfigured before connecting.
    </p>
    <button
      onclick={() =>
        actions.bulkAction(
          () => api.resetProfile(),
          "Reset profile",
          "reset-profile",
        )}
      disabled={actions.inFlightAction === "reset-profile"}
      class="px-3 py-1.5 text-xs font-medium rounded-md text-status-warn border border-status-warn-border bg-status-warn-bg cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
    >
      {actions.inFlightAction === "reset-profile"
        ? "Resetting..."
        : "Reset Profile to Defaults"}
    </button>
  </Card>

  <!-- Factory Reset -->
  <Card
    title="Factory Reset"
    borderStatus="err"
    helpSection="factory_reset"
    helpContext="receiver-admin"
  >
    <p class="text-sm text-text-muted m-0 mb-4">
      Clear <strong>all</strong> local data: profile, subscriptions, cursors, and
      epoch overrides. The receiver will disconnect and return to a fresh state. This
      cannot be undone.
    </p>
    {#if confirmingFactoryReset}
      <div class="flex items-center gap-3">
        <span class="text-sm text-status-err font-medium">Are you sure?</span>
        <button
          onclick={handleFactoryReset}
          disabled={actions.inFlightAction === "factory-reset"}
          class="px-3 py-1.5 text-xs font-medium rounded-md text-white bg-status-err border border-status-err cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {actions.inFlightAction === "factory-reset"
            ? "Resetting..."
            : "Yes, Factory Reset"}
        </button>
        <button
          onclick={() => (confirmingFactoryReset = false)}
          class="px-3 py-1.5 text-xs font-medium rounded-md text-text-secondary border border-border bg-surface-2 cursor-pointer hover:opacity-80"
        >
          Cancel
        </button>
      </div>
    {:else}
      <button
        onclick={() => (confirmingFactoryReset = true)}
        class="px-3 py-1.5 text-xs font-medium rounded-md text-status-err border border-status-err bg-transparent cursor-pointer hover:opacity-80"
      >
        Factory Reset...
      </button>
    {/if}
  </Card>
{/if}
