<script lang="ts">
  import {
    AnnouncerConfigForm,
    type AnnouncerConfigApi,
  } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";

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
</script>

<main class="max-w-[900px] mx-auto px-6 py-6">
  <h1 class="text-xl font-bold text-text-primary m-0 mb-6">Announcer</h1>
  <AnnouncerConfigForm api={announcerApi} />
</main>
