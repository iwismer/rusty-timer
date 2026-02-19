import type { ForwarderConfig, ForwarderStatus } from "./api";

export interface ConfigPageLoadState {
  config: ForwarderConfig | null;
  restartNeeded: boolean;
  loadError: string | null;
}

type GetConfig = () => Promise<ForwarderConfig>;
type GetStatus = () => Promise<ForwarderStatus>;

export async function loadConfigPageState(
  getConfig: GetConfig,
  getStatus: GetStatus,
): Promise<ConfigPageLoadState> {
  try {
    const [config, status] = await Promise.all([
      getConfig(),
      getStatus().catch(() => null),
    ]);

    return {
      config,
      restartNeeded: status?.restart_needed ?? false,
      loadError: null,
    };
  } catch (e) {
    return {
      config: null,
      restartNeeded: false,
      loadError: String(e),
    };
  }
}
