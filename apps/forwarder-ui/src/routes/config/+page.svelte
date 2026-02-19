<script lang="ts">
  import * as api from "$lib/api";
  import { loadConfigPageState } from "$lib/config-load";
  import { ForwarderConfig } from "@rusty-timer/shared-ui";
  import type { ConfigApi } from "@rusty-timer/shared-ui";

  const configApi: ConfigApi = {
    async getConfig() {
      const loaded = await loadConfigPageState(api.getConfig, api.getStatus);
      return {
        ok: !loaded.loadError,
        config: (loaded.config as unknown as Record<string, unknown>) ?? {},
        restart_needed: loaded.restartNeeded,
        error: loaded.loadError ?? undefined,
      };
    },
    async saveSection(section, data) {
      const result = await api.saveConfigSection(section, data);
      return {
        ok: result.ok,
        error: result.error,
        restart_needed: true,
      };
    },
    async restart() {
      try {
        await api.restart();
        return { ok: true };
      } catch (e) {
        return { ok: false, error: String(e) };
      }
    },
  };
</script>

<main class="max-w-[900px] mx-auto px-6 py-6">
  <ForwarderConfig {configApi} />
</main>
