<script lang="ts">
  import { page } from "$app/stores";
  import * as api from "$lib/api";
  import type { ReadEntry, DedupMode, SortOrder } from "$lib/api";
  import { metricsStore, streamsStore } from "$lib/stores";
  import { Card } from "@rusty-timer/shared-ui";
  import ReadsTable from "$lib/components/ReadsTable.svelte";
  import { createLatestRequestGate } from "$lib/latestRequestGate";

  let forwarderId = $derived($page.params.forwarderId!);

  let reads: ReadEntry[] = $state([]);
  let readsTotal = $state(0);
  let readsLoading = $state(false);
  let readsDedup: DedupMode = $state("none");
  let readsWindowSecs = $state(5);
  let readsLimit = $state(100);
  let readsOffset = $state(0);
  let readsOrder: SortOrder = $state("desc");
  const readsRequestGate = createLatestRequestGate();

  let forwarderStreamIds = $derived(
    $streamsStore
      .filter((s) => s.forwarder_id === forwarderId)
      .map((s) => s.stream_id),
  );
  let forwarderMetricsSignature = $derived(
    forwarderStreamIds
      .map((streamId) => {
        const m = $metricsStore[streamId];
        return `${streamId}:${m?.epoch_raw_count ?? -1}:${m?.epoch_last_received_at ?? ""}`;
      })
      .join("|"),
  );

  // Load reads on mount and re-fetch when new data arrives (metrics update via SSE)
  let readsInitialized = false;
  $effect(() => {
    forwarderMetricsSignature;
    void loadReads(forwarderId, readsInitialized);
    readsInitialized = true;
  });

  async function loadReads(fwdId: string, silent = false): Promise<void> {
    const token = readsRequestGate.next();
    if (!silent) readsLoading = true;
    try {
      const resp = await api.getForwarderReads(fwdId, {
        dedup: readsDedup,
        window_secs: readsWindowSecs,
        limit: readsLimit,
        offset: readsOffset,
        order: readsOrder,
      });
      if (!readsRequestGate.isLatest(token)) return;
      reads = resp.reads;
      readsTotal = resp.total;
    } catch {
      if (!readsRequestGate.isLatest(token)) return;
      reads = [];
      readsTotal = 0;
    } finally {
      if (!readsRequestGate.isLatest(token)) return;
      readsLoading = false;
    }
  }

  function handleReadsParamsChange() {
    void loadReads(forwarderId, true);
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
      bind:order={readsOrder}
      onParamsChange={handleReadsParamsChange}
    />
  </Card>
</main>
