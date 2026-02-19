import { apiFetch } from "@rusty-timer/shared-ui/lib/api-helpers";

export interface ReaderStatus {
  ip: string;
  state: "connected" | "connecting" | "disconnected";
  reads_session: number;
  reads_total: number;
  last_seen_secs: number | null;
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
  uplink?: {
    batch_mode?: string;
    batch_flush_ms?: number;
    batch_max_events?: number;
  };
  readers?: Array<{
    target?: string;
    enabled?: boolean;
    local_fallback_port?: number;
  }>;
}

export interface UpdateStatusResponse {
  status: "up_to_date" | "available" | "downloaded" | "failed";
  version?: string;
  error?: string;
}

export async function getStatus(): Promise<ForwarderStatus> {
  return apiFetch<ForwarderStatus>("/api/v1/status");
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

export async function resetEpoch(
  readerIp: string,
): Promise<{ new_epoch: number }> {
  return apiFetch(`/api/v1/streams/${readerIp}/reset-epoch`, {
    method: "POST",
  });
}

export async function getUpdateStatus(): Promise<UpdateStatusResponse> {
  return apiFetch<UpdateStatusResponse>("/update/status");
}

export async function applyUpdate(): Promise<void> {
  const resp = await fetch("/update/apply", { method: "POST" });
  if (resp.status !== 200) throw new Error(`apply update -> ${resp.status}`);
}
