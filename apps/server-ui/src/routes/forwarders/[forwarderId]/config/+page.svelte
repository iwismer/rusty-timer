<script lang="ts">
  import { page } from "$app/stores";
  import * as api from "$lib/api";
  import { streamsStore } from "$lib/stores";
  import { ForwarderConfigPage } from "@rusty-timer/shared-ui";
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
    async restartService() {
      return api.restartForwarderService(forwarderId);
    },
    async restartDevice() {
      return api.restartForwarderDevice(forwarderId);
    },
    async shutdownDevice() {
      return api.shutdownForwarderDevice(forwarderId);
    },
  };
</script>

<ForwarderConfigPage
  {configApi}
  {displayName}
  {isOnline}
  backHref="/"
  backLabel="Back to streams"
/>
