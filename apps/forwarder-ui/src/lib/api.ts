import { apiFetch, ApiError } from "@rusty-timer/shared-ui/lib/api-helpers";

export interface HardwareInfo {
  fw_version: string;
  hw_code: number;
  reader_id: number;
  config3: number;
}

export interface Config3Info {
  mode: "raw" | "event" | "fsls";
  timeout: number;
}

export interface TtoState {
  enabled: boolean;
}

export interface ClockInfo {
  reader_clock: string;
  drift_ms: number;
}

export interface ReaderInfo {
  banner?: string | null;
  hardware?: HardwareInfo | null;
  config?: Config3Info | null;
  tto_enabled?: boolean | null;
  clock?: ClockInfo | null;
  estimated_stored_reads?: number | null;
  recording?: boolean | null;
  connect_failures: number;
}

export interface ReaderStatus {
  ip: string;
  state: "connected" | "connecting" | "disconnected";
  reads_session: number;
  reads_total: number;
  last_seen_secs: number | null;
  local_port: number;
  current_epoch_name?: string | null;
  reader_info?: ReaderInfo | null;
}

export interface ForwarderStatus {
  forwarder_id: string;
  version: string;
  ready: boolean;
  ready_reason: string | null;
  uplink_connected: boolean;
  restart_needed: boolean;
  readers: ReaderStatus[];
}

export interface ForwarderConfig {
  schema_version?: number;
  display_name?: string;
  server?: {
    base_url?: string;
    forwarders_ws_path?: string;
  };
  auth?: {
    token_file?: string;
  };
  journal?: {
    sqlite_path?: string;
    prune_watermark_pct?: number;
  };
  status_http?: {
    bind?: string;
  };
  control?: {
    allow_power_actions?: boolean;
  };
  uplink?: {
    batch_mode?: string;
    batch_flush_ms?: number;
    batch_max_events?: number;
  };
  update?: {
    mode?: string;
  };
  readers?: Array<{
    target?: string;
    enabled?: boolean;
    local_fallback_port?: number;
  }>;
}

export interface LogsResponse {
  entries: string[];
}

export interface UpdateStatusResponse {
  status: "up_to_date" | "available" | "downloaded" | "failed";
  version?: string;
  error?: string;
}

export async function getStatus(): Promise<ForwarderStatus> {
  return apiFetch<ForwarderStatus>("/api/v1/status");
}

export async function getLogs(): Promise<LogsResponse> {
  return apiFetch<LogsResponse>("/api/v1/logs");
}

export async function getConfig(): Promise<ForwarderConfig> {
  return apiFetch<ForwarderConfig>("/api/v1/config");
}

export async function saveConfigSection(
  section: string,
  data: Record<string, unknown>,
): Promise<{ ok: boolean; error?: string }> {
  return apiFetch(`/api/v1/config/${section}`, {
    method: "POST",
    body: JSON.stringify(data),
  });
}

export async function restart(): Promise<void> {
  await apiFetch("/api/v1/restart", { method: "POST" });
}

export async function restartService(): Promise<{
  ok: boolean;
  error?: string;
}> {
  return apiFetch<{ ok: boolean; error?: string }>(
    "/api/v1/control/restart-service",
    { method: "POST" },
  );
}

export async function restartDevice(): Promise<{
  ok: boolean;
  error?: string;
}> {
  return apiFetch<{ ok: boolean; error?: string }>(
    "/api/v1/control/restart-device",
    { method: "POST" },
  );
}

export async function shutdownDevice(): Promise<{
  ok: boolean;
  error?: string;
}> {
  return apiFetch<{ ok: boolean; error?: string }>(
    "/api/v1/control/shutdown-device",
    { method: "POST" },
  );
}

export async function resetEpoch(
  readerIp: string,
): Promise<{ new_epoch: number }> {
  return apiFetch(`/api/v1/streams/${readerIp}/reset-epoch`, {
    method: "POST",
  });
}

export async function setCurrentEpochName(
  readerIp: string,
  name: string | null,
): Promise<void> {
  await apiFetch(`/api/v1/streams/${readerIp}/current-epoch/name`, {
    method: "PUT",
    body: JSON.stringify({ name }),
  });
}

export async function getUpdateStatus(): Promise<UpdateStatusResponse> {
  return apiFetch<UpdateStatusResponse>("/update/status");
}

export async function applyUpdate(): Promise<void> {
  const resp = await fetch("/update/apply", { method: "POST" });
  if (resp.status !== 200) throw new Error(`apply update -> ${resp.status}`);
}

export async function checkForUpdate(): Promise<UpdateStatusResponse> {
  return apiFetch<UpdateStatusResponse>("/update/check", {
    method: "POST",
  });
}

export async function downloadUpdate(): Promise<UpdateStatusResponse> {
  const resp = await fetch("/update/download", { method: "POST" });
  if (resp.status !== 200 && resp.status !== 409) {
    const text = await resp.text();
    throw new Error(`download update -> ${resp.status}: ${text}`);
  }
  return (await resp.json()) as UpdateStatusResponse;
}

export async function getReaderInfo(
  ip: string,
): Promise<ReaderInfo | undefined> {
  return apiFetch<ReaderInfo | undefined>(`/api/v1/readers/${ip}/info`);
}

export async function syncReaderClock(
  ip: string,
): Promise<{ reader_clock: string; clock_drift_ms: number | null }> {
  return apiFetch(`/api/v1/readers/${ip}/sync-clock`, { method: "POST" });
}

export async function getReadMode(
  ip: string,
): Promise<{ mode: "raw" | "event" | "fsls"; timeout: number }> {
  return apiFetch(`/api/v1/readers/${ip}/read-mode`);
}

export async function setReadMode(
  ip: string,
  mode: "raw" | "event" | "fsls",
  timeout = 5,
): Promise<{ mode: Config3Info["mode"] }> {
  return apiFetch(`/api/v1/readers/${ip}/read-mode`, {
    method: "PUT",
    body: JSON.stringify({ mode, timeout }),
  });
}

export async function getTtoState(ip: string): Promise<TtoState> {
  return apiFetch<TtoState>(`/api/v1/readers/${ip}/tto`);
}

export async function setTtoState(
  ip: string,
  enabled: boolean,
): Promise<TtoState> {
  return apiFetch<TtoState>(`/api/v1/readers/${ip}/tto`, {
    method: "PUT",
    body: JSON.stringify({ enabled }),
  });
}

export async function refreshReader(ip: string): Promise<ReaderInfo> {
  return apiFetch<ReaderInfo>(`/api/v1/readers/${ip}/refresh`, {
    method: "POST",
  });
}

export async function clearReaderRecords(ip: string): Promise<{ ok: boolean }> {
  return apiFetch(`/api/v1/readers/${ip}/clear-records`, { method: "POST" });
}

export interface DownloadReadsResponse {
  status: "started";
  estimated_reads: number;
}

export async function startDownloadReads(
  ip: string,
): Promise<DownloadReadsResponse> {
  try {
    return await apiFetch<DownloadReadsResponse>(
      `/api/v1/readers/${ip}/download-reads`,
      { method: "POST" },
    );
  } catch (err: unknown) {
    if (err instanceof ApiError && err.status === 409) {
      throw new Error("Download already in progress");
    }
    throw err;
  }
}

export async function setRecording(
  ip: string,
  enabled: boolean,
): Promise<{ recording: boolean }> {
  return apiFetch(`/api/v1/readers/${ip}/recording`, {
    method: "PUT",
    body: JSON.stringify({ enabled }),
  });
}

export async function reconnectReader(ip: string): Promise<{ ok: boolean }> {
  return apiFetch(`/api/v1/readers/${ip}/reconnect`, { method: "POST" });
}
