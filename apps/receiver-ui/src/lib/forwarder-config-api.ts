import type { ConfigApi } from "@rusty-timer/shared-ui";
import * as api from "./api";

export function createForwarderConfigApi(forwarderId: string): ConfigApi {
  return {
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
}
