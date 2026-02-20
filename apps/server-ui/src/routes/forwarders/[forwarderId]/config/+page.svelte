<script lang="ts">
  import { page } from "$app/stores";
  import * as api from "$lib/api";
  import { streamsStore } from "$lib/stores";
  import { ForwarderConfig } from "@rusty-timer/shared-ui";
  import type { ConfigApi } from "@rusty-timer/shared-ui";

  const forwarderId = $page.params.forwarderId!;

  let isOnline = $derived(
    $streamsStore.some((s) => s.forwarder_id === forwarderId && s.online),
  );
  let displayName = $derived(
    $streamsStore.find((s) => s.forwarder_id === forwarderId)
      ?.forwarder_display_name ?? forwarderId,
  );

  const configApi: ConfigApi = {
    async getConfig() {
      const resp = await api.getForwarderConfig(forwarderId);
      return {
        ok: resp.ok,
        config: resp.config ?? {},
        restart_needed: resp.restart_needed,
        error: resp.error ?? undefined,
      };
    },
    async saveSection(section, data) {
      const result = await api.setForwarderConfig(forwarderId, section, data);
      return {
        ok: result.ok,
        error: result.error ?? undefined,
        restart_needed: result.restart_needed,
      };
    },
    async restart() {
      return await api.restartForwarder(forwarderId);
    },
  };
</script>

<main class="max-w-[900px] mx-auto px-6 py-6">
  <div class="mb-4">
    <a href="/" class="text-xs text-accent no-underline hover:underline">
      &larr; Back to streams
    </a>
  </div>
  <ForwarderConfig {configApi} {displayName} {isOnline} />
</main>
