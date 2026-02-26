<script lang="ts">
  import { onMount } from "svelte";
  import { Card } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";
  import type { StreamEntry } from "$lib/api";

  let loading = $state(true);
  let loadError: string | null = $state(null);
  let saving = $state(false);
  let saveError: string | null = $state(null);
  let saveSuccess: string | null = $state(null);
  let resetting = $state(false);
  let resetStatus: string | null = $state(null);

  let streams: StreamEntry[] = $state([]);
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
      const [streamsResp, config] = await Promise.all([
        api.getStreams(),
        api.getAnnouncerConfig(),
      ]);
      streams = streamsResp.streams;
      enabled = config.enabled;
      selectedStreamIds = [...config.selected_stream_ids];
      maxListSize = config.max_list_size;
      enabledUntil = config.enabled_until;
      updatedAt = config.updated_at;
    } catch (err) {
      loadError = String(err);
    } finally {
      loading = false;
    }
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
      const updated = await api.updateAnnouncerConfig({
        enabled,
        selected_stream_ids: selectedStreamIds,
        max_list_size: maxListSize,
      });
      enabled = updated.enabled;
      selectedStreamIds = [...updated.selected_stream_ids];
      maxListSize = updated.max_list_size;
      enabledUntil = updated.enabled_until;
      updatedAt = updated.updated_at;
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
      await api.resetAnnouncer();
      resetStatus = "Announcer runtime reset.";
    } catch (err) {
      resetStatus = String(err);
    } finally {
      resetting = false;
    }
  }

  function streamLabel(stream: StreamEntry): string {
    return stream.display_alias || stream.reader_ip;
  }
</script>

<main class="max-w-[900px] mx-auto px-6 py-6">
  <div class="flex items-center justify-between gap-3 mb-6">
    <h1 class="text-xl font-bold text-text-primary m-0">Announcer</h1>
    <a
      href="/announcer"
      class="text-sm font-medium text-action-600 hover:text-action-700 underline"
    >
      Open announcer page
    </a>
  </div>

  {#if loading}
    <p class="text-sm text-text-muted">Loading announcer settings...</p>
  {:else if loadError}
    <p class="text-sm text-status-err">{loadError}</p>
  {:else}
    <Card>
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
          </label>
        </div>

        <div>
          <p class="text-sm font-medium text-text-primary m-0 mb-2">Streams</p>
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
          <button
            data-testid="announcer-reset-btn"
            onclick={() => void handleReset()}
            disabled={resetting}
            class="px-3 py-1 text-xs font-medium rounded-md bg-status-warn-bg border border-status-warn-border text-status-warn cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {resetting ? "Resetting..." : "Reset announcer"}
          </button>
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
</main>
