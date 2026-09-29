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
  let availableEpochs = $state<number[]>([]);
  let loadingEpochs = $state(false);
  let exporting = $state(false);
  let feedback = $state<{ kind: "ok" | "err"; message: string } | null>(null);

  let streams = $derived(store.streams?.streams ?? []);

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
    const streamArg =
      selectedTarget === "merged" || selectedTarget === "separate"
        ? null
        : selectedTarget;

    loadingEpochs = true;
    api
      .getExportEpochs(streamArg)
      .then((res) => {
        availableEpochs = res.epochs;
      })
      .catch(() => {
        availableEpochs = [];
      })
      .finally(() => {
        loadingEpochs = false;
      });
  });

  async function handleDownload() {
    exporting = true;
    feedback = null;
    try {
      const mode = selectedTarget === "separate" ? "separate" : "merged";
      const stream_id =
        selectedTarget === "merged" || selectedTarget === "separate"
          ? undefined
          : selectedTarget;
      const epoch = selectedEpoch === "all" ? undefined : Number(selectedEpoch);
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
        // If saveFileDialog is unavailable (e.g. running in browser/test), fallback to direct browser download
        targetPath = null;
      }

      const res = await api.exportTagdata({
        stream_id,
        epoch,
        mode,
        destination_dir: targetPath ?? undefined,
      });

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
          <div class="flex flex-col gap-2 max-h-40 overflow-y-auto pr-1">
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
            <option value="all">All Reads (all epochs)</option>
            {#each availableEpochs as ep}
              <option value={String(ep)}>
                Epoch {ep}
              </option>
            {/each}
          </select>
        </div>

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
