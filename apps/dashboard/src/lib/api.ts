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
  return apiFetch<StreamMetrics>(`/api/v1/streams/${streamId}/metrics`);
}

/** Returns the href for the export.raw streaming download (no fetch needed — direct link). */
export function exportRawUrl(streamId: string): string {
  return `${BASE}/api/v1/streams/${streamId}/export.raw`;
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
