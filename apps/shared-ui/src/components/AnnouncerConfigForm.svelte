<script lang="ts">
  import { onMount } from "svelte";
  import type {
    AnnouncerConfigApi,
    AnnouncerConfig,
    AnnouncerStreamEntry,
  } from "../lib/announcer-types";
  import Card from "./Card.svelte";
  import HelpTip from "./HelpTip.svelte";

  let { api }: { api: AnnouncerConfigApi } = $props();

  let loading = $state(true);
  let loadError: string | null = $state(null);
  let saving = $state(false);
  let saveError: string | null = $state(null);
  let saveSuccess: string | null = $state(null);
  let resetting = $state(false);
  let resetStatus: string | null = $state(null);

  let streams: AnnouncerStreamEntry[] = $state([]);
  let enabled = $state(false);
  let selectedStreamIds: string[] = $state([]);
  let maxListSize = $state(25);
  let enabledUntil: string | null = $state(null);
  let updatedAt: string | null = $state(null);

  const canSave = $derived(
    !saving && (!enabled || selectedStreamIds.length > 0),
  );

  onMount(() => {
    void load();
  });

  async function load() {
    loading = true;
    loadError = null;
    saveError = null;
    saveSuccess = null;
    resetStatus = null;
    try {
      const [streamList, config] = await Promise.all([
        api.getStreams(),
        api.getConfig(),
      ]);
      streams = streamList;
      applyConfig(config);
    } catch (err) {
      loadError = String(err);
    } finally {
      loading = false;
    }
  }

  function applyConfig(config: AnnouncerConfig) {
    enabled = config.enabled;
    selectedStreamIds = [...config.selected_stream_ids];
    maxListSize = config.max_list_size;
    enabledUntil = config.enabled_until;
    updatedAt = config.updated_at;
  }

  function toggleStream(streamId: string, checked: boolean) {
    if (checked) {
      if (!selectedStreamIds.includes(streamId)) {
        selectedStreamIds = [...selectedStreamIds, streamId];
      }
      return;
    }
    selectedStreamIds = selectedStreamIds.filter((id) => id !== streamId);
  }

  async function handleSave() {
    if (!canSave) return;
    saving = true;
    saveError = null;
    saveSuccess = null;
    try {
      const updated = await api.saveConfig({
        enabled,
        selected_stream_ids: selectedStreamIds,
        max_list_size: maxListSize,
      });
      applyConfig(updated);
      saveSuccess = "Saved announcer settings.";
    } catch (err) {
      saveError = String(err);
    } finally {
      saving = false;
    }
  }

  async function handleReset() {
    resetting = true;
    resetStatus = null;
    try {
      await api.reset();
      resetStatus = "Announcer runtime reset.";
    } catch (err) {
      resetStatus = String(err);
    } finally {
      resetting = false;
    }
  }

  function streamLabel(stream: AnnouncerStreamEntry): string {
    return stream.display_alias || stream.reader_ip;
  }
</script>

{#if loading}
  <p class="text-sm text-text-muted">Loading announcer settings...</p>
{:else if loadError}
  <p class="text-sm text-status-err">{loadError}</p>
{:else}
  <Card
    title="Announcer Configuration"
    helpSection="announcer"
    helpContext="server"
  >
    <div class="space-y-5">
      <div class="flex items-center gap-2">
        <input
          id="announcer-enabled"
          type="checkbox"
          bind:checked={enabled}
          class="cursor-pointer"
        />
        <label for="announcer-enabled" class="text-sm text-text-primary">
          Enable announcer
          <HelpTip
            fieldKey="enabled"
            sectionKey="announcer"
            context="server"
          />
        </label>
      </div>

      <div>
        <p class="text-sm font-medium text-text-primary m-0 mb-2">
          Streams
          <HelpTip
            fieldKey="streams"
            sectionKey="announcer"
            context="server"
          />
        </p>
        {#if streams.length === 0}
          <p class="text-sm text-text-muted m-0">
            No streams available yet. Connect a forwarder first.
          </p>
        {:else}
          <div class="grid gap-2">
            {#each streams as stream (stream.stream_id)}
              <label
                class="flex items-center gap-2 text-sm text-text-primary"
              >
                <input
                  type="checkbox"
                  checked={selectedStreamIds.includes(stream.stream_id)}
                  onchange={(e) =>
                    toggleStream(stream.stream_id, e.currentTarget.checked)}
                  class="cursor-pointer"
                />
                <span>{streamLabel(stream)}</span>
                <span class="text-text-muted">({stream.forwarder_id})</span>
              </label>
            {/each}
          </div>
        {/if}
      </div>

      <div>
        <label
          for="announcer-max-list-size"
          class="text-sm text-text-primary"
        >
          Max list size
          <HelpTip
            fieldKey="max_list_size"
            sectionKey="announcer"
            context="server"
          />
        </label>
        <input
          id="announcer-max-list-size"
          type="number"
          min="1"
          max="500"
          bind:value={maxListSize}
          class="mt-1 w-28 px-2 py-1 text-sm rounded-md border border-border bg-surface-0 text-text-primary"
        />
      </div>

      {#if enabled && selectedStreamIds.length === 0}
        <p class="text-xs text-status-err m-0">
          Select at least one stream to enable announcer mode.
        </p>
      {/if}

      <div class="flex items-center gap-2">
        <button
          data-testid="announcer-save-btn"
          onclick={() => void handleSave()}
          disabled={!canSave}
          class="px-3 py-1 text-xs font-medium rounded-md bg-surface-2 border border-border text-text-secondary cursor-pointer hover:bg-surface-3 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {saving ? "Saving..." : "Save"}
        </button>
        <span class="inline-flex items-center gap-1">
          <button
            data-testid="announcer-reset-btn"
            onclick={() => void handleReset()}
            disabled={resetting}
            class="px-3 py-1 text-xs font-medium rounded-md bg-status-warn-bg border border-status-warn-border text-status-warn cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {resetting ? "Resetting..." : "Reset announcer"}</button
          ><HelpTip
            fieldKey="reset"
            sectionKey="announcer"
            context="server"
          />
        </span>
      </div>

      {#if enabledUntil}
        <p class="text-xs text-text-muted m-0">
          Enabled until: {new Date(enabledUntil).toLocaleString()}
        </p>
      {/if}
      {#if updatedAt}
        <p class="text-xs text-text-muted m-0">
          Last updated: {new Date(updatedAt).toLocaleString()}
        </p>
      {/if}
      {#if saveSuccess}
        <p class="text-xs text-status-ok m-0">{saveSuccess}</p>
      {/if}
      {#if saveError}
        <p class="text-xs text-status-err m-0">{saveError}</p>
      {/if}
      {#if resetStatus}
        <p class="text-xs text-text-muted m-0">{resetStatus}</p>
      {/if}
    </div>
  </Card>
{/if}
