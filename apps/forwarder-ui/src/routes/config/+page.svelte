<script lang="ts">
  import * as api from "$lib/api";
  import { loadConfigPageState } from "$lib/config-load";
  import { mapSaveSectionResult } from "$lib/config-api-adapter";
  import { ForwarderConfigPage } from "@rusty-timer/shared-ui";
  import type { ConfigApi } from "@rusty-timer/shared-ui";

  async function runControlAction(
    action: () => Promise<{ ok: boolean; error?: string }>,
  ): Promise<{ ok: boolean; error?: string }> {
    try {
      return await action();
    } catch (e) {
      return { ok: false, error: String(e) };
    }
  }

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
      return mapSaveSectionResult(result);
    },
    async restartService() {
      return runControlAction(() => api.restartService());
    },
    async restartDevice() {
      return runControlAction(() => api.restartDevice());
    },
    async shutdownDevice() {
      return runControlAction(() => api.shutdownDevice());
    },
  };
</script>

<ForwarderConfigPage {configApi} pageTitle="Config" />
