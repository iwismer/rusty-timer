// Dashboard - Server HTTP API client
// All dashboard-to-server communication goes through this module exclusively.
// Base URL defaults to the same origin (dashboard is served by the server process).

const BASE = typeof window !== "undefined" ? "" : "http://localhost:8080";

// ----- Types -----

export interface StreamEntry {
  stream_id: string;
  forwarder_id: string;
  reader_ip: string;
  display_alias: string | null;
  forwarder_display_name: string | null;
  online: boolean;
  stream_epoch: number;
  created_at: string;
}

export interface StreamsResponse {
  streams: StreamEntry[];
}

export interface StreamMetrics {
  raw_count: number;
  dedup_count: number;
  retransmit_count: number;
  /** Milliseconds since last canonical event, or null if no events yet. */
  lag: number | null;
  backlog: number;
  epoch_raw_count: number;
  epoch_dedup_count: number;
  epoch_retransmit_count: number;
  /** Milliseconds since last canonical event in current epoch, or null. */
  epoch_lag: number | null;
  /** ISO 8601 timestamp of last event in current epoch, or null. */
  epoch_last_received_at: string | null;
  unique_chips: number;
}

export interface ApiError {
  code: string;
  message: string;
  details?: Record<string, unknown>;
}

// ----- Internal helpers -----

async function apiFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const resp = await fetch(`${BASE}${path}`, {
    headers: { "Content-Type": "application/json", ...(init?.headers ?? {}) },
    ...init,
  });
  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(
      `API ${init?.method ?? "GET"} ${path} -> ${resp.status}: ${text}`,
    );
  }
  if (resp.status === 204) return undefined as unknown as T;
  return resp.json();
}

// ----- Public API -----

/** GET /api/v1/streams */
export async function getStreams(): Promise<StreamsResponse> {
  return apiFetch<StreamsResponse>("/api/v1/streams");
}

/** PATCH /api/v1/streams/{stream_id} — update display alias */
export async function renameStream(
  streamId: string,
  displayAlias: string,
): Promise<StreamEntry> {
  return apiFetch<StreamEntry>(`/api/v1/streams/${streamId}`, {
    method: "PATCH",
    body: JSON.stringify({ display_alias: displayAlias }),
  });
}

/** GET /api/v1/streams/{stream_id}/metrics */
export async function getMetrics(streamId: string): Promise<StreamMetrics> {
  const data = await apiFetch<Record<string, unknown>>(
    `/api/v1/streams/${streamId}/metrics`,
  );
  return {
    raw_count: data.raw_count as number,
    dedup_count: data.dedup_count as number,
    retransmit_count: data.retransmit_count as number,
    lag: (data.lag_ms as number | null) ?? null,
    backlog: 0,
    epoch_raw_count: data.epoch_raw_count as number,
    epoch_dedup_count: data.epoch_dedup_count as number,
    epoch_retransmit_count: data.epoch_retransmit_count as number,
    epoch_lag: (data.epoch_lag_ms as number | null) ?? null,
    epoch_last_received_at:
      (data.epoch_last_received_at as string | null) ?? null,
    unique_chips: data.unique_chips as number,
  };
}

/** Returns the href for the export.txt streaming download (no fetch needed — direct link). */
export function exportRawUrl(streamId: string): string {
  return `${BASE}/api/v1/streams/${streamId}/export.txt`;
}

/** Returns the href for the export.csv streaming download (no fetch needed — direct link). */
export function exportCsvUrl(streamId: string): string {
  return `${BASE}/api/v1/streams/${streamId}/export.csv`;
}

/** POST /api/v1/streams/{stream_id}/reset-epoch
 *  Resolves on 204. Throws on 404, 409, or 5xx. */
export async function resetEpoch(streamId: string): Promise<void> {
  return apiFetch<void>(`/api/v1/streams/${streamId}/reset-epoch`, {
    method: "POST",
  });
}

// ----- Forwarder config types -----

export interface ForwarderConfigResponse {
  ok: boolean;
  error: string | null;
  config: Record<string, unknown>;
  restart_needed: boolean;
}

export interface ConfigSetResult {
  ok: boolean;
  error: string | null;
  restart_needed: boolean;
}

// ----- Forwarder config API -----

/** GET /api/v1/forwarders/{forwarderId}/config */
export async function getForwarderConfig(
  forwarderId: string,
): Promise<ForwarderConfigResponse> {
  return apiFetch<ForwarderConfigResponse>(
    `/api/v1/forwarders/${encodeURIComponent(forwarderId)}/config`,
  );
}

/** POST /api/v1/forwarders/{forwarderId}/config/{section} */
export async function setForwarderConfig(
  forwarderId: string,
  section: string,
  payload: Record<string, unknown>,
): Promise<ConfigSetResult> {
  return apiFetch<ConfigSetResult>(
    `/api/v1/forwarders/${encodeURIComponent(forwarderId)}/config/${encodeURIComponent(section)}`,
    {
      method: "POST",
      body: JSON.stringify(payload),
    },
  );
}

/** POST /api/v1/forwarders/{forwarderId}/restart */
export async function restartForwarder(
  forwarderId: string,
): Promise<{ ok: boolean; error?: string }> {
  return apiFetch<{ ok: boolean; error?: string }>(
    `/api/v1/forwarders/${encodeURIComponent(forwarderId)}/restart`,
    { method: "POST" },
  );
}
