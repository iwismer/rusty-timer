<script lang="ts">
  import { onMount } from "svelte";
  import { store, selectRace, loadRaceDetail } from "$lib/store.svelte";
  import * as api from "$lib/api";
  import type { ParticipantEntry } from "$lib/api";
  import { Card, HelpDialog } from "@rusty-timer/shared-ui";

  // --- List view state ---
  let newRaceName = $state("");
  let creating = $state(false);
  let createError: string | null = $state(null);
  let listLoading = $state(true);
  let listError: string | null = $state(null);
  let confirmingDeleteId: string | null = $state(null);
  let deleting: Record<string, boolean> = $state({});
  let deleteError: Record<string, string | null> = $state({});
  let racesHelpOpen = $state(false);

  // --- Detail view state ---
  let pplFile: File | null = $state(null);
  let bibchipFile: File | null = $state(null);
  let uploadingPpl = $state(false);
  let uploadingBibchip = $state(false);
  let uploadResult: string | null = $state(null);

  // --- Sort state ---
  let sortField: keyof ParticipantEntry = $state("bib");
  let sortAsc = $state(true);

  // --- Filter state ---
  let filterText = $state("");

  // Reset detail-view state when selected race changes
  let prevRaceId: string | null = null;
  $effect(() => {
    const currentId = store.selectedRaceId;
    if (currentId !== prevRaceId) {
      prevRaceId = currentId;
      pplFile = null;
      bibchipFile = null;
      uploadResult = null;
      filterText = "";
      sortField = "bib";
      sortAsc = true;
    }
  });

  // Derived: filtered + sorted participants
  let displayParticipants = $derived.by(() => {
    let result = store.raceParticipants ?? [];

    // Filter
    if (filterText.trim()) {
      const q = filterText.trim().toLowerCase();
      result = result.filter((p) => {
        return (
          String(p.bib).includes(q) ||
          p.first_name.toLowerCase().includes(q) ||
          p.last_name.toLowerCase().includes(q) ||
          p.gender.toLowerCase().includes(q) ||
          (p.affiliation ?? "").toLowerCase().includes(q) ||
          p.chip_ids.some((c) => c.toLowerCase().includes(q))
        );
      });
    }

    // Sort
    const field = sortField;
    const asc = sortAsc;
    result = [...result].sort((a, b) => {
      let aVal: string | number;
      let bVal: string | number;

      if (field === "chip_ids") {
        aVal = a.chip_ids.join(", ");
        bVal = b.chip_ids.join(", ");
      } else if (field === "affiliation") {
        aVal = a.affiliation ?? "";
        bVal = b.affiliation ?? "";
      } else {
        aVal = a[field];
        bVal = b[field];
      }

      if (typeof aVal === "number" && typeof bVal === "number") {
        return asc ? aVal - bVal : bVal - aVal;
      }
      const cmp = String(aVal).localeCompare(String(bVal));
      return asc ? cmp : -cmp;
    });

    return result;
  });

  // Stats
  let totalParticipants = $derived((store.raceParticipants ?? []).length);
  let totalChips = $derived(
    (store.raceParticipants ?? []).reduce(
      (sum, p) => sum + p.chip_ids.length,
      0,
    ),
  );
  let unmatchedCount = $derived((store.raceUnmatchedChips ?? []).length);

  onMount(() => {
    void loadRaces();
  });

  async function loadRaces() {
    listLoading = true;
    listError = null;
    try {
      const resp = await api.getRaces();
      store.races = resp.races;
    } catch (e) {
      listError = String(e);
    } finally {
      listLoading = false;
    }
  }

  async function handleCreate() {
    if (!newRaceName.trim()) return;
    creating = true;
    createError = null;
    try {
      const created = await api.createRace(newRaceName.trim());
      store.races = [created, ...store.races];
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
      store.races = store.races.filter((r) => r.race_id !== raceId);
      confirmingDeleteId = null;
    } catch (e) {
      deleteError[raceId] = String(e);
    } finally {
      deleting[raceId] = false;
    }
  }

  const MAX_UPLOAD_SIZE = 10 * 1024 * 1024; // 10 MB

  async function fileToBase64(file: File): Promise<string> {
    if (file.size > MAX_UPLOAD_SIZE) {
      throw new Error(
        `File too large (${(file.size / 1024 / 1024).toFixed(1)}MB). Maximum size is 10MB.`,
      );
    }
    const buffer = await file.arrayBuffer();
    const bytes = new Uint8Array(buffer);
    let binary = "";
    for (let i = 0; i < bytes.length; i++) {
      binary += String.fromCharCode(bytes[i]);
    }
    return btoa(binary);
  }

  async function handleUploadPpl() {
    if (!pplFile || !store.selectedRaceId) return;
    uploadingPpl = true;
    uploadResult = null;
    try {
      const base64Data = await fileToBase64(pplFile);
      const result = await api.uploadRaceFile(
        store.selectedRaceId,
        "participants",
        base64Data,
        pplFile.name,
      );
      uploadResult = `Successfully imported ${result.imported} participants.`;
      pplFile = null;
      await loadRaceDetail(store.selectedRaceId);
      if (store.raceDetailError) {
        uploadResult = `Imported ${result.imported} participants, but failed to refresh the list. Try navigating back and reopening the race.`;
      }
    } catch (e) {
      uploadResult = `Error: ${String(e)}`;
    } finally {
      uploadingPpl = false;
    }
  }

  async function handleUploadBibchip() {
    if (!bibchipFile || !store.selectedRaceId) return;
    uploadingBibchip = true;
    uploadResult = null;
    try {
      const base64Data = await fileToBase64(bibchipFile);
      const result = await api.uploadRaceFile(
        store.selectedRaceId,
        "chips",
        base64Data,
        bibchipFile.name,
      );
      uploadResult = `Successfully imported ${result.imported} chip mappings.`;
      bibchipFile = null;
      await loadRaceDetail(store.selectedRaceId);
      if (store.raceDetailError) {
        uploadResult = `Imported ${result.imported} chip mappings, but failed to refresh the list. Try navigating back and reopening the race.`;
      }
    } catch (e) {
      uploadResult = `Error: ${String(e)}`;
    } finally {
      uploadingBibchip = false;
    }
  }

  function toggleSort(field: keyof ParticipantEntry) {
    if (sortField === field) {
      sortAsc = !sortAsc;
    } else {
      sortField = field;
      sortAsc = true;
    }
  }

  function sortIndicator(field: keyof ParticipantEntry): string {
    if (sortField !== field) return "";
    return sortAsc ? " \u25B2" : " \u25BC";
  }

  function formatDate(iso: string): string {
    const d = new Date(iso);
    if (isNaN(d.getTime())) return "(unknown date)";
    return d.toLocaleString();
  }
</script>

<div class="h-full flex flex-col">
  {#if store.selectedRaceId !== null}
    <!-- Detail view -->
    <div class="flex-1 overflow-y-auto">
      <div class="max-w-[1100px] mx-auto px-6 py-6">
        <!-- Back link -->
        <div class="mb-4">
          <button
            class="text-xs text-accent bg-transparent border-none cursor-pointer hover:underline"
            onclick={() => selectRace(null)}
          >
            &larr; Back to races
          </button>
        </div>

        <!-- Heading -->
        <h1 class="text-xl font-bold text-text-primary m-0 mb-6">
          Race Detail
        </h1>

        <!-- Upload section -->
        <div class="grid grid-cols-1 md:grid-cols-2 gap-4 mb-6">
          <Card
            title="Upload Participants (.ppl)"
            helpSection="race_detail"
            helpContext="receiver"
          >
            <div class="flex flex-col gap-3">
              <input
                type="file"
                accept=".ppl"
                onchange={(e: Event) => {
                  const target = e.target as HTMLInputElement;
                  pplFile = target.files?.[0] ?? null;
                }}
                class="text-sm text-text-secondary file:mr-3 file:px-3 file:py-1.5 file:rounded-md file:border file:border-border file:bg-surface-2 file:text-text-secondary file:text-sm file:cursor-pointer file:font-medium"
              />
              <button
                onclick={() => void handleUploadPpl()}
                disabled={!pplFile || uploadingPpl}
                class="px-4 py-1.5 text-sm font-medium rounded-md bg-accent text-white cursor-pointer hover:opacity-90 disabled:opacity-50 disabled:cursor-not-allowed w-fit"
              >
                {uploadingPpl ? "Uploading..." : "Upload"}
              </button>
            </div>
          </Card>

          <Card
            title="Upload Chip Mappings (.bibchip)"
            helpSection="race_detail"
            helpContext="receiver"
          >
            <div class="flex flex-col gap-3">
              <input
                type="file"
                accept=".txt,.csv,.bibchip"
                onchange={(e: Event) => {
                  const target = e.target as HTMLInputElement;
                  bibchipFile = target.files?.[0] ?? null;
                }}
                class="text-sm text-text-secondary file:mr-3 file:px-3 file:py-1.5 file:rounded-md file:border file:border-border file:bg-surface-2 file:text-text-secondary file:text-sm file:cursor-pointer file:font-medium"
              />
              <button
                onclick={() => void handleUploadBibchip()}
                disabled={!bibchipFile || uploadingBibchip}
                class="px-4 py-1.5 text-sm font-medium rounded-md bg-accent text-white cursor-pointer hover:opacity-90 disabled:opacity-50 disabled:cursor-not-allowed w-fit"
              >
                {uploadingBibchip ? "Uploading..." : "Upload"}
              </button>
            </div>
          </Card>
        </div>

        <!-- Upload result message -->
        {#if uploadResult}
          <p
            class="text-sm mb-4 m-0 {uploadResult.startsWith('Error')
              ? 'text-status-err'
              : 'text-status-ok'}"
          >
            {uploadResult}
          </p>
        {/if}

        <!-- Loading / Error states for detail -->
        {#if store.raceDetailLoading}
          <p class="text-sm text-text-muted">Loading participants...</p>
        {:else if store.raceDetailError}
          <p class="text-sm text-status-err">{store.raceDetailError}</p>
        {:else}
          <!-- Stats bar -->
          <div class="flex items-center gap-6 mb-4 text-sm text-text-secondary">
            <span>
              <span class="font-medium text-text-primary"
                >{totalParticipants}</span
              > participants
            </span>
            <span>
              <span class="font-medium text-text-primary">{totalChips}</span> chips
            </span>
            <span>
              <span
                class="font-medium {unmatchedCount > 0
                  ? 'text-status-err'
                  : 'text-text-primary'}"
              >
                {unmatchedCount}
              </span> unmatched chips
            </span>
          </div>

          <!-- Filter input -->
          <div class="mb-4">
            <input
              type="text"
              bind:value={filterText}
              placeholder="Filter by bib, name, gender, team, chip ID..."
              class="w-full max-w-md px-3 py-1.5 text-sm rounded-md border border-border bg-surface-0 text-text-primary placeholder:text-text-muted focus:outline-none focus:border-accent"
            />
          </div>

          <!-- Participants table -->
          {#if (store.raceParticipants ?? []).length === 0}
            <p class="text-sm text-text-muted">
              No participants yet. Upload a .ppl file to get started.
            </p>
          {:else if displayParticipants.length === 0}
            <p class="text-sm text-text-muted">
              No participants match the current filter.
            </p>
          {:else}
            <div
              class="overflow-y-auto rounded-md border border-border"
              style="max-height: 600px;"
            >
              <table class="w-full text-sm border-collapse">
                <thead class="sticky top-0 bg-surface-1 z-10">
                  <tr>
                    <th
                      onclick={() => toggleSort("bib")}
                      class="px-3 py-2 text-left text-xs font-medium text-text-muted uppercase tracking-wider cursor-pointer hover:text-text-primary select-none border-b border-border"
                    >
                      Bib{sortIndicator("bib")}
                    </th>
                    <th
                      onclick={() => toggleSort("first_name")}
                      class="px-3 py-2 text-left text-xs font-medium text-text-muted uppercase tracking-wider cursor-pointer hover:text-text-primary select-none border-b border-border"
                    >
                      First Name{sortIndicator("first_name")}
                    </th>
                    <th
                      onclick={() => toggleSort("last_name")}
                      class="px-3 py-2 text-left text-xs font-medium text-text-muted uppercase tracking-wider cursor-pointer hover:text-text-primary select-none border-b border-border"
                    >
                      Last Name{sortIndicator("last_name")}
                    </th>
                    <th
                      onclick={() => toggleSort("gender")}
                      class="px-3 py-2 text-left text-xs font-medium text-text-muted uppercase tracking-wider cursor-pointer hover:text-text-primary select-none border-b border-border"
                    >
                      Gender{sortIndicator("gender")}
                    </th>
                    <th
                      onclick={() => toggleSort("affiliation")}
                      class="px-3 py-2 text-left text-xs font-medium text-text-muted uppercase tracking-wider cursor-pointer hover:text-text-primary select-none border-b border-border"
                    >
                      Team{sortIndicator("affiliation")}
                    </th>
                    <th
                      onclick={() => toggleSort("chip_ids")}
                      class="px-3 py-2 text-left text-xs font-medium text-text-muted uppercase tracking-wider cursor-pointer hover:text-text-primary select-none border-b border-border"
                    >
                      Chip ID(s){sortIndicator("chip_ids")}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {#each displayParticipants as p (p.bib)}
                    <tr class="border-b border-border hover:bg-surface-1">
                      <td class="px-3 py-2 font-mono">{p.bib}</td>
                      <td class="px-3 py-2">{p.first_name}</td>
                      <td class="px-3 py-2">{p.last_name}</td>
                      <td class="px-3 py-2">{p.gender}</td>
                      <td class="px-3 py-2">{p.affiliation ?? ""}</td>
                      <td class="px-3 py-2 font-mono"
                        >{p.chip_ids.join(", ")}</td
                      >
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>

            <p class="text-xs text-text-muted mt-2 m-0">
              Showing {displayParticipants.length} of {(
                store.raceParticipants ?? []
              ).length} participants
            </p>
          {/if}

          <!-- Unmatched chips section -->
          {#if (store.raceUnmatchedChips ?? []).length > 0}
            <div class="mt-6">
              <Card title="Unmatched Chips">
                <p class="text-xs text-text-muted mb-3 m-0">
                  These chip mappings reference bib numbers that don't match any
                  participant.
                </p>
                <div class="overflow-y-auto rounded-md border border-border">
                  <table class="w-full text-sm border-collapse">
                    <thead class="sticky top-0 bg-surface-1 z-10">
                      <tr>
                        <th
                          class="px-3 py-2 text-left text-xs font-medium text-text-muted uppercase tracking-wider border-b border-border"
                        >
                          Bib
                        </th>
                        <th
                          class="px-3 py-2 text-left text-xs font-medium text-text-muted uppercase tracking-wider border-b border-border"
                        >
                          Chip ID
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {#each store.raceUnmatchedChips ?? [] as chip (chip.chip_id)}
                        <tr class="border-b border-border hover:bg-surface-1">
                          <td class="px-3 py-2 font-mono">{chip.bib}</td>
                          <td class="px-3 py-2 font-mono">{chip.chip_id}</td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                </div>
              </Card>
            </div>
          {/if}
        {/if}
      </div>
    </div>
  {:else}
    <!-- List view -->
    <div class="flex-1 overflow-y-auto">
      <div class="max-w-[1100px] mx-auto px-6 py-6">
        <div class="flex items-center justify-between mb-6">
          <span class="inline-flex items-center gap-2">
            <h1 class="text-xl font-bold text-text-primary m-0">Races</h1>
            <button
              onclick={() => {
                racesHelpOpen = true;
              }}
              class="inline-flex items-center justify-center w-5 h-5 rounded-full border border-border text-text-muted hover:text-accent hover:border-accent text-xs font-bold cursor-pointer bg-transparent transition-colors"
              aria-label="Help for Races"
              type="button">?</button
            >
          </span>
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
        {#if listLoading}
          <p class="text-sm text-text-muted">Loading races...</p>
        {:else if listError}
          <p class="text-sm text-status-err">{listError}</p>
        {:else if store.races.length === 0}
          <p class="text-sm text-text-muted">No races found.</p>
        {:else}
          <!-- Race list -->
          <div class="grid gap-3">
            {#each store.races as race (race.race_id)}
              <Card>
                <div class="flex items-center justify-between">
                  <div class="flex-1 min-w-0">
                    <div class="flex items-center gap-3 mb-1">
                      <button
                        onclick={() => selectRace(race.race_id)}
                        class="text-sm font-semibold text-accent bg-transparent border-none cursor-pointer p-0 hover:underline text-left"
                      >
                        {race.name}
                      </button>
                    </div>
                    <div
                      class="flex items-center gap-3 text-xs text-text-muted"
                    >
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
                        class="px-2 py-1 text-xs font-medium rounded-md bg-status-err-bg border border-status-err-border text-status-err cursor-pointer hover:opacity-80"
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
      </div>
    </div>
  {/if}
</div>

<HelpDialog
  open={racesHelpOpen}
  sectionKey="races"
  context="receiver"
  onClose={() => {
    racesHelpOpen = false;
  }}
/>
