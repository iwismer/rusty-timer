export interface ConfigApi {
  getConfig(): Promise<ConfigLoadResult>;
  saveSection(
    section: string,
    data: Record<string, unknown>,
  ): Promise<ConfigSaveResult>;
  restart(): Promise<ConfigRestartResult>;
}

export interface ConfigLoadResult {
  ok: boolean;
  config: Record<string, unknown>;
  restart_needed: boolean;
  error?: string;
}

export interface ConfigSaveResult {
  ok: boolean;
  error?: string;
  restart_needed: boolean;
}

export interface ConfigRestartResult {
  ok: boolean;
  error?: string;
}
