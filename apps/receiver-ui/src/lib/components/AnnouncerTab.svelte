<script lang="ts">
  import {
    AnnouncerConfigForm,
    type AnnouncerConfigApi,
  } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";
  import { store } from "$lib/store.svelte";

  const announcerApi: AnnouncerConfigApi = {
    async getStreams() {
      const resp = await api.getServerStreams();
      return resp.streams.map((s) => ({
        stream_id: s.stream_id,
        forwarder_id: s.forwarder_id,
        reader_ip: s.reader_ip,
        display_alias: s.display_alias,
      }));
    },
    getConfig: () => api.getAnnouncerConfig(),
    saveConfig: (update) => api.putAnnouncerConfig(update),
    reset: () => api.resetAnnouncer(),
  };

  function announcerPageUrl(): string | null {
    const wsUrl = store.savedServerUrl;
    if (!wsUrl) return null;
    try {
      const url = new URL(wsUrl);
      url.protocol = url.protocol === "wss:" ? "https:" : "http:";
      url.pathname = "/announcer";
      return url.href;
    } catch (err) {
      console.warn(
        "Cannot derive announcer page URL from server URL:",
        wsUrl,
        err,
      );
      return null;
    }
  }

  async function openAnnouncerPage() {
    const url = announcerPageUrl();
    if (!url) return;
    try {
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl(url);
    } catch (err) {
      if ((window as any).__TAURI_INTERNALS__) {
        console.error("Failed to open announcer page:", err);
      } else {
        // Fallback for non-Tauri (e.g. dev server in browser)
        window.open(url, "_blank");
      }
    }
  }
</script>

<main class="max-w-[900px] mx-auto px-6 py-6">
  <div class="flex items-center justify-between gap-3 mb-6">
    <h1 class="text-xl font-bold text-text-primary m-0">Announcer</h1>
    {#if announcerPageUrl()}
      <button
        onclick={() => void openAnnouncerPage()}
        class="text-sm font-medium text-action-600 hover:text-action-700 underline cursor-pointer bg-transparent border-none p-0"
      >
        Open announcer page
      </button>
    {/if}
  </div>

  <AnnouncerConfigForm api={announcerApi} />
</main>
