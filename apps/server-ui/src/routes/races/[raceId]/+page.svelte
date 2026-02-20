<script lang="ts">
  import { onMount } from "svelte";
  import { page } from "$app/stores";
  import * as api from "$lib/api";
  import type { ParticipantEntry, UnmatchedChip } from "$lib/api";
  import { Card } from "@rusty-timer/shared-ui";

  // Route param
  let raceId = $derived($page.params.raceId!);

  // Data state
  let participants: ParticipantEntry[] = $state([]);
  let unmatchedChips: UnmatchedChip[] = $state([]);
  let loading = $state(true);
  let error: string | null = $state(null);

  // File upload state
  let pplFile: File | null = $state(null);
  let bibchipFile: File | null = $state(null);
  let uploadingPpl = $state(false);
  let uploadingBibchip = $state(false);
  let uploadResult: string | null = $state(null);

  // Sort state
  let sortField: keyof ParticipantEntry = $state("bib");
  let sortAsc = $state(true);

  // Filter state
  let filterText = $state("");

  // Derived: filtered + sorted participants
  let displayParticipants = $derived.by(() => {
    let result = participants;

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
  let totalParticipants = $derived(participants.length);
  let totalChips = $derived(
    participants.reduce((sum, p) => sum + p.chip_ids.length, 0),
  );
  let unmatchedCount = $derived(unmatchedChips.length);

  // Load data
  onMount(() => {
    void loadParticipants();
  });

  async function loadParticipants() {
    loading = true;
    error = null;
    try {
      const resp = await api.getParticipants(raceId);
      participants = resp.participants;
      unmatchedChips = resp.chips_without_participant;
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  // Upload handlers
  async function handleUploadPpl() {
    if (!pplFile) return;
    uploadingPpl = true;
    uploadResult = null;
    try {
      const result = await api.uploadParticipants(raceId, pplFile);
      uploadResult = `Successfully imported ${result.imported} participants.`;
      pplFile = null;
      await loadParticipants();
    } catch (e) {
      uploadResult = `Error: ${String(e)}`;
    } finally {
      uploadingPpl = false;
    }
  }

  async function handleUploadBibchip() {
    if (!bibchipFile) return;
    uploadingBibchip = true;
    uploadResult = null;
    try {
      const result = await api.uploadChips(raceId, bibchipFile);
      uploadResult = `Successfully imported ${result.imported} chip mappings.`;
      bibchipFile = null;
      await loadParticipants();
    } catch (e) {
      uploadResult = `Error: ${String(e)}`;
    } finally {
      uploadingBibchip = false;
    }
  }

  // Sort helpers
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
</script>

<main class="max-w-[1100px] mx-auto px-6 py-6">
  <!-- Back link -->
  <div class="mb-4">
    <a href="/races" class="text-xs text-accent no-underline hover:underline">
      &larr; Back to races
    </a>
  </div>

  <!-- Heading -->
  <h1 class="text-xl font-bold text-text-primary m-0 mb-6">Race Detail</h1>

  <!-- Upload section -->
  <div class="grid grid-cols-2 gap-4 mb-6">
    <Card title="Upload Participants (.ppl)">
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

    <Card title="Upload Chip Mappings (.bibchip)">
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

  <!-- Loading / Error states -->
  {#if loading}
    <p class="text-sm text-text-muted">Loading participants...</p>
  {:else if error}
    <p class="text-sm text-status-err">{error}</p>
  {:else}
    <!-- Stats bar -->
    <div class="flex items-center gap-6 mb-4 text-sm text-text-secondary">
      <span>
        <span class="font-medium text-text-primary">{totalParticipants}</span> participants
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
    {#if participants.length === 0}
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
                <td class="px-3 py-2 font-mono">{p.chip_ids.join(", ")}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>

      <p class="text-xs text-text-muted mt-2 m-0">
        Showing {displayParticipants.length} of {participants.length} participants
      </p>
    {/if}

    <!-- Unmatched chips section -->
    {#if unmatchedChips.length > 0}
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
                {#each unmatchedChips as chip (chip.chip_id)}
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
</main>
