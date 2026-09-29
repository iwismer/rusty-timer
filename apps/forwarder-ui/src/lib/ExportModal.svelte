<script lang="ts">
  import * as api from "$lib/api";
  import type { ReaderStatus } from "$lib/api";

  interface Props {
    open: boolean;
    readers: ReaderStatus[];
    initialReaderIp?: string | null;
    onclose: () => void;
  }

  let { open, readers, initialReaderIp = null, onclose }: Props = $props();

  let selectedTarget = $state<string>("merged");
  let selectedEpoch = $state<string>("all");
  let availableEpochs = $state<number[]>([]);
  let loadingEpochs = $state(false);

  $effect(() => {
    if (!open) return;
    if (initialReaderIp && readers.some((r) => r.ip === initialReaderIp)) {
      selectedTarget = initialReaderIp;
    } else {
      selectedTarget = "merged";
    }
    selectedEpoch = "all";
  });

  $effect(() => {
    if (!open) return;
    const readerArg =
      selectedTarget === "merged" || selectedTarget === "separate"
        ? undefined
        : selectedTarget;

    loadingEpochs = true;
    api
      .getExportEpochs(readerArg)
      .then((eps) => {
        availableEpochs = eps;
      })
      .catch(() => {
        availableEpochs = [];
      })
      .finally(() => {
        loadingEpochs = false;
      });
  });

  async function handleDownload() {
    let reader: string | undefined;
    let mode: "merged" | "separate" | undefined;

    if (selectedTarget === "merged") {
      reader = "all";
      mode = "merged";
    } else if (selectedTarget === "separate") {
      reader = "all";
      mode = "separate";
    } else {
      reader = selectedTarget;
      mode = "merged";
    }

    const url = api.getExportTagdataUrl({
      reader,
      epoch: selectedEpoch === "all" ? undefined : selectedEpoch,
      mode,
    });
    const filename = mode === "separate" ? "TAGDATA.ZIP" : "TAGDATA.TXT";

    if (typeof window !== "undefined" && "showSaveFilePicker" in window) {
      try {
        const picker = (
          window as unknown as {
            showSaveFilePicker: (
              opts: unknown,
            ) => Promise<FileSystemFileHandle>;
          }
        ).showSaveFilePicker;
        const handle = await picker({
          suggestedName: filename,
          types: [
            mode === "separate"
              ? {
                  description: "ZIP archive",
                  accept: { "application/zip": [".zip"] },
                }
              : {
                  description: "TAGDATA file",
                  accept: { "text/plain": [".txt", ".TXT"] },
                },
          ],
        });
        const resp = await fetch(url);
        const blob = await resp.blob();
        const writable = await handle.createWritable();
        await writable.write(blob);
        await writable.close();
        onclose();
        return;
      } catch (err: unknown) {
        if (
          err &&
          typeof err === "object" &&
          "name" in err &&
          err.name === "AbortError"
        ) {
          return;
        }
      }
    }

    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);

    onclose();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      onclose();
    }
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
      class="w-full max-w-md rounded-lg border border-border bg-surface-1 p-5 shadow-xl text-text-primary"
    >
      <div
        class="flex items-center justify-between pb-3 border-b border-border"
      >
        <h3 id="export-modal-title" class="text-base font-semibold m-0">
          Export TAGDATA.TXT
        </h3>
        <button
          onclick={onclose}
          class="text-text-muted hover:text-text-primary text-lg leading-none cursor-pointer bg-transparent border-none p-1"
          aria-label="Close dialog"
        >
          &times;
        </button>
      </div>

      <div class="py-4 flex flex-col gap-4 text-sm">
        <div>
          <span class="block text-xs font-medium text-text-muted mb-2">
            Readers
          </span>
          <div class="flex flex-col gap-2">
            {#if readers.length > 1}
              <label class="flex items-center gap-2 cursor-pointer">
                <input
                  type="radio"
                  name="export-target"
                  value="merged"
                  bind:group={selectedTarget}
                  class="accent-accent"
                />
                <span>All Readers (Merged TAGDATA.TXT)</span>
              </label>
              <label class="flex items-center gap-2 cursor-pointer">
                <input
                  type="radio"
                  name="export-target"
                  value="separate"
                  bind:group={selectedTarget}
                  class="accent-accent"
                />
                <span>All Readers (Separate files in TAGDATA.ZIP)</span>
              </label>
            {/if}

            {#each readers as r}
              <label class="flex items-center gap-2 cursor-pointer font-mono">
                <input
                  type="radio"
                  name="export-target"
                  value={r.ip}
                  bind:group={selectedTarget}
                  class="accent-accent"
                />
                <span>{r.ip}</span>
              </label>
            {/each}
          </div>
        </div>

        <div>
          <label
            for="export-epoch-select"
            class="block text-xs font-medium text-text-muted mb-1"
          >
            Scope
          </label>
          <select
            id="export-epoch-select"
            bind:value={selectedEpoch}
            class="w-full text-xs px-2.5 py-1.5 rounded-md border border-border bg-surface-0 text-text-primary"
            disabled={loadingEpochs}
          >
            <option value="all">All Reads (all epochs)</option>
            {#each availableEpochs as ep}
              <option value={String(ep)}>
                Epoch {ep}
                {#each readers as r}
                  {#if r.current_epoch === ep && r.current_epoch_name}
                    ({r.current_epoch_name})
                  {/if}
                {/each}
              </option>
            {/each}
          </select>
        </div>

        <p class="text-xs text-text-muted m-0">
          Exports raw reader passes with Windows CRLF endings, matching Race
          Director's standard IPICO tag file format.
        </p>
      </div>

      <div class="flex justify-end gap-2 pt-3 border-t border-border">
        <button
          onclick={onclose}
          class="px-3 py-1.5 text-xs font-medium rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2"
        >
          Cancel
        </button>
        <button
          onclick={handleDownload}
          class="px-3 py-1.5 text-xs font-medium rounded-md text-white bg-accent border-none cursor-pointer hover:bg-accent-hover"
        >
          Download {selectedTarget === "separate"
            ? "TAGDATA.ZIP"
            : "TAGDATA.TXT"}
        </button>
      </div>
    </div>
  </div>
{/if}
