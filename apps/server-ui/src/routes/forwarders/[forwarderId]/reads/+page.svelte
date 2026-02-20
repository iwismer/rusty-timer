<script lang="ts">
  import { page } from "$app/stores";
  import * as api from "$lib/api";
  import type { ReadEntry, DedupMode } from "$lib/api";
  import { Card } from "@rusty-timer/shared-ui";
  import ReadsTable from "$lib/components/ReadsTable.svelte";

  let forwarderId = $derived($page.params.forwarderId!);

  let reads: ReadEntry[] = $state([]);
  let readsTotal = $state(0);
  let readsLoading = $state(false);
  let readsDedup: DedupMode = $state("none");
  let readsWindowSecs = $state(5);
  let readsLimit = $state(100);
  let readsOffset = $state(0);

  $effect(() => {
    void loadReads(forwarderId);
  });

  async function loadReads(fwdId: string): Promise<void> {
    readsLoading = true;
    try {
      const resp = await api.getForwarderReads(fwdId, {
        dedup: readsDedup,
        window_secs: readsWindowSecs,
        limit: readsLimit,
        offset: readsOffset,
      });
      reads = resp.reads;
      readsTotal = resp.total;
    } catch {
      reads = [];
      readsTotal = 0;
    } finally {
      readsLoading = false;
    }
  }

  function handleReadsParamsChange() {
    void loadReads(forwarderId);
  }
</script>

<main class="max-w-[1100px] mx-auto px-6 py-6">
  <div class="mb-4">
    <a href="/" class="text-xs text-accent no-underline hover:underline">
      &larr; Back to stream list
    </a>
  </div>

  <h1 class="text-xl font-bold text-text-primary mb-6">
    Reads — {forwarderId}
  </h1>

  <Card title="All Reads">
    <ReadsTable
      {reads}
      total={readsTotal}
      loading={readsLoading}
      showStreamColumn={true}
      bind:dedup={readsDedup}
      bind:windowSecs={readsWindowSecs}
      bind:limit={readsLimit}
      bind:offset={readsOffset}
      onParamsChange={handleReadsParamsChange}
    />
  </Card>
</main>
