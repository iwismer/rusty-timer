<script lang="ts">
  import { save as saveFileDialog } from "@tauri-apps/plugin-dialog";
  import * as api from "$lib/api";
  import { store, streamIdentity } from "$lib/store.svelte";
  import { btnPrimary, btnSecondary } from "$lib/ui-classes";

  interface Props {
    open: boolean;
    initialStreamId?: string | null;
    onclose: () => void;
  }

  let { open, initialStreamId = null, onclose }: Props = $props();

  let selectedTarget = $state<string>("merged");
  let selectedEpoch = $state<string>("all");
  let selectedStreamEpochs = $state<Record<string, string>>({});
  let availableEpochDetails = $state<api.ExportEpochInfo[]>([]);
  let backendStreamsMap = $state<Map<string, api.ExportEpochInfo[]>>(new Map());
  let loadingEpochs = $state(false);
  let exporting = $state(false);
  let feedback = $state<{ kind: "ok" | "err"; message: string } | null>(null);

  let streams = $derived(store.streams?.streams ?? []);

  interface ExportStreamItem {
    key: string;
    label: string;
    forwarder_id?: string | null;
    epochs: api.ExportEpochInfo[];
    totalReads: number;
  }

  let exportStreamsList = $derived.by<ExportStreamItem[]>(() => {
    const list: ExportStreamItem[] = [];
    const seen = new Set<string>();

    for (const s of streams) {
      const key = streamIdentity(s);
      seen.add(key);
      const rawEpochs = backendStreamsMap.get(key) ?? [];
      const enrichedEpochs = rawEpochs.map((ep) => {
        const name =
          ep.name ||
          s.epoch_options?.find((o) => o.stream_epoch === ep.epoch)?.name ||
          (s.stream_epoch === ep.epoch ? s.current_epoch_name : null) ||
          null;
        return { ...ep, name };
      });
      const totalReads = enrichedEpochs.reduce(
        (acc, e) => acc + e.read_count,
        0,
      );
      list.push({
        key,
        label: s.reader_ip ?? s.display_alias ?? s.stream_id,
        forwarder_id: s.forwarder_id,
        epochs: enrichedEpochs,
        totalReads: totalReads > 0 ? totalReads : (s.reads_total ?? 0),
      });
    }

    for (const [key, rawEpochs] of backendStreamsMap.entries()) {
      if (seen.has(key)) continue;
      const totalReads = rawEpochs.reduce((acc, e) => acc + e.read_count, 0);
      list.push({
        key,
        label: key,
        forwarder_id: null,
        epochs: rawEpochs,
        totalReads,
      });
    }

    return list;
  });

  let singleStreamEpochs = $derived.by<api.ExportEpochInfo[]>(() => {
    if (selectedTarget === "merged" || selectedTarget === "separate") {
      return availableEpochDetails;
    }
    const fromMap = exportStreamsList.find((s) => s.key === selectedTarget);
    if (fromMap && fromMap.epochs.length > 0) {
      return fromMap.epochs;
    }
    return availableEpochDetails;
  });

  let singleStreamTotalReads = $derived.by<number>(() => {
    if (selectedTarget === "merged" || selectedTarget === "separate") {
      return exportStreamsList.reduce((acc, s) => acc + s.totalReads, 0);
    }
    const item = exportStreamsList.find((s) => s.key === selectedTarget);
    return item?.totalReads ?? 0;
  });

  $effect(() => {
    if (!open) {
      feedback = null;
      return;
    }
    if (
      initialStreamId &&
      streams.some((s) => streamIdentity(s) === initialStreamId)
    ) {
      selectedTarget = initialStreamId;
    } else {
      selectedTarget = "merged";
    }
    selectedEpoch = "all";
    feedback = null;
  });

  $effect(() => {
    if (!open) return;
    const isAllStreams =
      selectedTarget === "merged" || selectedTarget === "separate";
    const streamArg = isAllStreams ? null : selectedTarget;

    loadingEpochs = true;
    api
      .getExportEpochs(streamArg)
      .then((res) => {
        availableEpochDetails =
          res.details ??
          res.epochs.map((ep) => ({
            epoch: ep,
            name: null,
            read_count: 0,
          }));

        if (res.streams && res.streams.length > 0) {
          const map = new Map<string, api.ExportEpochInfo[]>();
          for (const s of res.streams) {
            map.set(s.stream_id, s.epochs);
          }
          backendStreamsMap = map;
        } else if (!isAllStreams && streamArg) {
          backendStreamsMap.set(streamArg, availableEpochDetails);
          backendStreamsMap = new Map(backendStreamsMap);
        }

        // Initialize selected stream epochs if not present
        const nextStreamEpochs = { ...selectedStreamEpochs };
        for (const s of streams) {
          const key = streamIdentity(s);
          if (!nextStreamEpochs[key]) {
            nextStreamEpochs[key] = "all";
          }
        }
        selectedStreamEpochs = nextStreamEpochs;
      })
      .catch(() => {
        availableEpochDetails = [];
      })
      .finally(() => {
        loadingEpochs = false;
      });
  });

  function setAllStreamEpochs(target: "all" | "latest") {
    const next: Record<string, string> = {};
    for (const item of exportStreamsList) {
      if (target === "all" || item.epochs.length === 0) {
        next[item.key] = "all";
      } else {
        next[item.key] = String(item.epochs[0].epoch);
      }
    }
    selectedStreamEpochs = next;
  }

  function formatEpochOption(ep: api.ExportEpochInfo): string {
    const countStr = `${ep.read_count.toLocaleString()} ${ep.read_count === 1 ? "read" : "reads"}`;
    if (ep.name && ep.name.trim().length > 0) {
      return `Epoch ${ep.epoch} (${ep.name.trim()}) — ${countStr}`;
    }
    return `Epoch ${ep.epoch} — ${countStr}`;
  }

  async function handleDownload() {
    exporting = true;
    feedback = null;
    try {
      const mode = selectedTarget === "separate" ? "separate" : "merged";
      const isAllStreams =
        selectedTarget === "merged" || selectedTarget === "separate";

      const defaultFilename =
        mode === "separate" ? "TAGDATA.ZIP" : "TAGDATA.TXT";
      const defaultDir = store.editRdImportDir?.trim() || "";
      const defaultPath = defaultDir
        ? `${defaultDir.replace(/[\\/]+$/, "")}/${defaultFilename}`
        : defaultFilename;

      let targetPath: string | null = null;
      try {
        const chosen = await saveFileDialog({
          defaultPath,
          filters: [
            mode === "separate"
              ? { name: "Zip archive", extensions: ["zip", "ZIP"] }
              : { name: "TAGDATA file", extensions: ["txt", "TXT"] },
          ],
        });
        if (typeof chosen === "string") {
          targetPath = chosen;
        } else if (chosen === null) {
          // User cancelled the file picker dialog
          exporting = false;
          return;
        }
      } catch {
        targetPath = null;
      }

      let res: api.ExportTagdataResponse;
      if (isAllStreams) {
        const stream_epochs: Record<string, number | null> = {};
        for (const item of exportStreamsList) {
          const choice = selectedStreamEpochs[item.key] ?? "all";
          stream_epochs[item.key] = choice === "all" ? null : Number(choice);
        }
        res = await api.exportTagdata({
          mode,
          stream_epochs,
          destination_dir: targetPath ?? undefined,
        });
      } else {
        const epoch =
          selectedEpoch === "all" ? undefined : Number(selectedEpoch);
        res = await api.exportTagdata({
          stream_id: selectedTarget,
          epoch,
          mode: "merged",
          destination_dir: targetPath ?? undefined,
        });
      }

      if (targetPath) {
        feedback = {
          kind: "ok",
          message: `Saved ${res.read_count.toLocaleString()} reads to ${targetPath}.`,
        };
      } else {
        if (res.content) {
          const blob = new Blob([res.content], {
            type: "text/plain;charset=utf-8",
          });
          const url = URL.createObjectURL(blob);
          const a = document.createElement("a");
          a.href = url;
          a.download = res.filename;
          document.body.appendChild(a);
          a.click();
          document.body.removeChild(a);
          URL.revokeObjectURL(url);
        } else if (res.zip_base64) {
          const a = document.createElement("a");
          a.href = `data:application/zip;base64,${res.zip_base64}`;
          a.download = res.filename;
          document.body.appendChild(a);
          a.click();
          document.body.removeChild(a);
        }

        feedback = {
          kind: "ok",
          message: `Exported ${res.read_count.toLocaleString()} reads to ${res.filename}.`,
        };
      }
    } catch (e) {
      feedback = {
        kind: "err",
        message: `Export failed: ${e instanceof Error ? e.message : String(e)}`,
      };
    } finally {
      exporting = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") onclose();
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-xs p-4"
    onclick={(e) => {
      if (e.target === e.currentTarget) onclose();
    }}
  >
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="export-modal-title"
      class="bg-surface-1 border border-border rounded-lg shadow-xl w-full max-w-lg p-5 flex flex-col gap-4 text-text-primary"
    >
      <div
        class="flex items-center justify-between pb-3 border-b border-border"
      >
        <h3 id="export-modal-title" class="text-base font-semibold m-0">
          Export TAGDATA
        </h3>
        <button
          onclick={onclose}
          class="text-text-muted hover:text-text-primary text-lg leading-none cursor-pointer bg-transparent border-none p-1"
          aria-label="Close"
        >
          &times;
        </button>
      </div>

      {#if feedback}
        <div
          class="text-xs p-3 rounded border {feedback.kind === 'ok'
            ? 'bg-status-ok-bg text-status-ok border-status-ok-border'
            : 'bg-status-err-bg text-status-err border-status-err-border'}"
        >
          {feedback.message}
        </div>
      {/if}

      <div class="flex flex-col gap-4 text-sm">
        <div>
          <span class="block text-xs font-medium text-text-muted mb-2">
            Target Stream
          </span>
          <div class="flex flex-col gap-2 max-h-36 overflow-y-auto pr-1">
            {#if streams.length > 1 || streams.length === 0}
              <label class="flex items-center gap-2 cursor-pointer text-xs">
                <input
                  type="radio"
                  name="receiver-export-target"
                  value="merged"
                  bind:group={selectedTarget}
                  class="accent-accent"
                />
                <span>All Streams (Merged TAGDATA.TXT)</span>
              </label>
              <label class="flex items-center gap-2 cursor-pointer text-xs">
                <input
                  type="radio"
                  name="receiver-export-target"
                  value="separate"
                  bind:group={selectedTarget}
                  class="accent-accent"
                />
                <span>All Streams (Separate files in TAGDATA.ZIP)</span>
              </label>
            {/if}

            {#each streams as s}
              {@const key = streamIdentity(s)}
              {@const label = s.reader_ip ?? s.display_alias ?? s.stream_id}
              <label
                class="flex items-center gap-2 cursor-pointer text-xs font-mono"
              >
                <input
                  type="radio"
                  name="receiver-export-target"
                  value={key}
                  bind:group={selectedTarget}
                  class="accent-accent"
                />
                <span>
                  {label}
                  {#if s.forwarder_id}
                    <span class="text-text-muted text-[11px] font-sans"
                      >({s.forwarder_id})</span
                    >
                  {/if}
                </span>
              </label>
            {/each}
          </div>
        </div>

        {#if selectedTarget === "merged" || selectedTarget === "separate"}
          <div>
            <div class="flex items-center justify-between mb-1.5">
              <span class="block text-xs font-medium text-text-muted">
                Scope (Epoch per Stream)
              </span>
              {#if exportStreamsList.length > 1}
                <div class="flex items-center gap-1.5 text-[11px]">
                  <span class="text-text-muted">Set all:</span>
                  <button
                    type="button"
                    class="text-accent hover:underline cursor-pointer bg-transparent border-none p-0"
                    onclick={() => setAllStreamEpochs("all")}
                    disabled={loadingEpochs || exporting}
                  >
                    All Reads
                  </button>
                  <span class="text-text-muted">|</span>
                  <button
                    type="button"
                    class="text-accent hover:underline cursor-pointer bg-transparent border-none p-0"
                    onclick={() => setAllStreamEpochs("latest")}
                    disabled={loadingEpochs || exporting}
                  >
                    Latest Epoch
                  </button>
                </div>
              {/if}
            </div>

            <div class="flex flex-col gap-2 max-h-48 overflow-y-auto pr-1">
              {#if exportStreamsList.length === 0}
                <select
                  id="receiver-export-epoch-select"
                  bind:value={selectedEpoch}
                  class="w-full text-xs px-2.5 py-1.5 rounded-md border border-border bg-surface-0 text-text-primary"
                  disabled={loadingEpochs || exporting}
                >
                  <option value="all">All Reads (all epochs)</option>
                  {#each availableEpochDetails as ep}
                    <option value={String(ep.epoch)}>
                      {formatEpochOption(ep)}
                    </option>
                  {/each}
                </select>
              {:else}
                {#each exportStreamsList as item (item.key)}
                  <div
                    class="flex flex-col gap-1 p-2 rounded bg-surface-0 border border-border"
                  >
                    <div class="flex items-center justify-between text-xs">
                      <span class="font-medium truncate font-mono"
                        >{item.label}</span
                      >
                      {#if item.forwarder_id}
                        <span class="text-[11px] text-text-muted"
                          >({item.forwarder_id})</span
                        >
                      {/if}
                    </div>
                    <select
                      id="receiver-export-epoch-stream-{item.key}"
                      data-testid="receiver-export-epoch-stream-{item.key}"
                      bind:value={selectedStreamEpochs[item.key]}
                      class="w-full text-xs px-2 py-1.5 rounded border border-border bg-surface-1 text-text-primary"
                      disabled={loadingEpochs || exporting}
                    >
                      <option value="all">
                        All Reads (all epochs){item.totalReads > 0
                          ? ` — ${item.totalReads.toLocaleString()} reads`
                          : ""}
                      </option>
                      {#each item.epochs as ep}
                        <option value={String(ep.epoch)}>
                          {formatEpochOption(ep)}
                        </option>
                      {/each}
                    </select>
                  </div>
                {/each}
              {/if}
            </div>
          </div>
        {:else}
          <div>
            <label
              for="receiver-export-epoch-select"
              class="block text-xs font-medium text-text-muted mb-1"
            >
              Scope
            </label>
            <select
              id="receiver-export-epoch-select"
              bind:value={selectedEpoch}
              class="w-full text-xs px-2.5 py-1.5 rounded-md border border-border bg-surface-0 text-text-primary"
              disabled={loadingEpochs || exporting}
            >
              <option value="all">
                All Reads (all epochs){singleStreamTotalReads > 0
                  ? ` — ${singleStreamTotalReads.toLocaleString()} reads`
                  : ""}
              </option>
              {#each singleStreamEpochs as ep}
                <option value={String(ep.epoch)}>
                  {formatEpochOption(ep)}
                </option>
              {/each}
            </select>
          </div>
        {/if}

        <p class="text-xs text-text-muted m-0">
          Exports raw reader passes with Windows CRLF endings, matching Race
          Director's standard IPICO tag file format.
        </p>
      </div>

      <div
        class="flex items-center justify-end gap-2 pt-3 border-t border-border"
      >
        <button onclick={onclose} class={btnSecondary} disabled={exporting}>
          Cancel
        </button>

        <button
          data-testid="export-download-btn"
          onclick={handleDownload}
          class={btnPrimary}
          disabled={exporting}
        >
          {exporting
            ? "Exporting\u2026"
            : `Download ${selectedTarget === "separate" ? "TAGDATA.ZIP" : "TAGDATA.TXT"}`}
        </button>
      </div>
    </div>
  </div>
{/if}
