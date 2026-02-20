<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "$lib/api";
  import type { RaceEntry } from "$lib/api";
  import { Card } from "@rusty-timer/shared-ui";

  // State
  let races: RaceEntry[] = $state([]);
  let loading = $state(true);
  let error: string | null = $state(null);

  // Create race form
  let newRaceName = $state("");
  let creating = $state(false);
  let createError: string | null = $state(null);

  // Delete confirmation (race_id of the race being confirmed, or null)
  let confirmingDeleteId: string | null = $state(null);
  let deleting: Record<string, boolean> = $state({});
  let deleteError: Record<string, string | null> = $state({});

  onMount(() => {
    void loadRaces();
  });

  async function loadRaces() {
    loading = true;
    error = null;
    try {
      const resp = await api.getRaces();
      races = resp.races;
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  async function handleCreate() {
    if (!newRaceName.trim()) return;
    creating = true;
    createError = null;
    try {
      const created = await api.createRace(newRaceName.trim());
      races = [created, ...races];
      newRaceName = "";
    } catch (e) {
      createError = String(e);
    } finally {
      creating = false;
    }
  }

  async function handleDelete(raceId: string) {
    deleting[raceId] = true;
    deleteError[raceId] = null;
    try {
      await api.deleteRace(raceId);
      races = races.filter((r) => r.race_id !== raceId);
      confirmingDeleteId = null;
    } catch (e) {
      deleteError[raceId] = String(e);
    } finally {
      deleting[raceId] = false;
    }
  }

  function formatDate(iso: string): string {
    return new Date(iso).toLocaleString();
  }
</script>

<main class="max-w-[1100px] mx-auto px-6 py-6">
  <div class="flex items-center justify-between mb-6">
    <h1 class="text-xl font-bold text-text-primary m-0">Races</h1>
  </div>

  <!-- Create Race form -->
  <div class="mb-6">
    <Card>
      <div class="flex gap-2 items-center">
        <input
          type="text"
          bind:value={newRaceName}
          placeholder="New race name"
          aria-label="New race name"
          onkeydown={(e: KeyboardEvent) => {
            if (e.key === "Enter") void handleCreate();
          }}
          class="flex-1 px-2 py-1 text-sm rounded-md border border-border bg-surface-0 text-text-primary placeholder:text-text-muted focus:outline-none focus:border-accent"
        />
        <button
          onclick={() => void handleCreate()}
          disabled={creating || !newRaceName.trim()}
          class="px-3 py-1 text-xs font-medium rounded-md bg-surface-2 border border-border text-text-secondary cursor-pointer hover:bg-surface-3 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {creating ? "Creating..." : "Create Race"}
        </button>
      </div>
      {#if createError}
        <p class="text-xs text-status-err mt-2 m-0">{createError}</p>
      {/if}
    </Card>
  </div>

  <!-- Loading state -->
  {#if loading}
    <p class="text-sm text-text-muted">Loading races...</p>
  {:else if error}
    <p class="text-sm text-status-err">{error}</p>
  {:else if races.length === 0}
    <p class="text-sm text-text-muted">No races found.</p>
  {:else}
    <!-- Race list -->
    <div class="grid gap-3">
      {#each races as race (race.race_id)}
        <Card>
          <div class="flex items-center justify-between">
            <div class="flex-1 min-w-0">
              <div class="flex items-center gap-3 mb-1">
                <a
                  href="/races/{race.race_id}"
                  class="text-sm font-semibold text-accent no-underline hover:underline"
                >
                  {race.name}
                </a>
              </div>
              <div class="flex items-center gap-3 text-xs text-text-muted">
                <span>
                  {race.participant_count}
                  {race.participant_count === 1
                    ? "participant"
                    : "participants"}
                </span>
                <span>&middot;</span>
                <span>
                  {race.chip_count}
                  {race.chip_count === 1 ? "chip" : "chips"}
                </span>
                <span>&middot;</span>
                <span>Created {formatDate(race.created_at)}</span>
              </div>
            </div>

            <div class="flex items-center gap-2 ml-4">
              {#if confirmingDeleteId === race.race_id}
                <span class="text-xs text-text-muted">Delete?</span>
                <button
                  onclick={() => void handleDelete(race.race_id)}
                  disabled={deleting[race.race_id]}
                  class="px-2 py-1 text-xs font-medium rounded-md bg-status-err-bg border border-status-err-border text-status-err cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  {deleting[race.race_id] ? "Deleting..." : "Yes"}
                </button>
                <button
                  onclick={() => (confirmingDeleteId = null)}
                  class="px-2 py-1 text-xs font-medium rounded-md bg-surface-2 border border-border text-text-secondary cursor-pointer hover:bg-surface-3"
                >
                  No
                </button>
              {:else}
                <button
                  onclick={() => (confirmingDeleteId = race.race_id)}
                  class="px-2 py-1 text-xs font-medium rounded-md bg-surface-2 border border-border text-text-secondary cursor-pointer hover:bg-surface-3"
                >
                  Delete
                </button>
              {/if}
            </div>
          </div>

          {#if deleteError[race.race_id]}
            <p class="text-xs text-status-err mt-2 m-0">
              {deleteError[race.race_id]}
            </p>
          {/if}
        </Card>
      {/each}
    </div>
  {/if}
</main>
