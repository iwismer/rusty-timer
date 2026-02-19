// Receiver UI - Control API client
// All UI-to-receiver communication goes through this module exclusively.
// Uses same-origin requests (UI is served by the receiver's axum server).

const BASE = "";

export interface Profile {
  server_url: string;
  token: string;
  log_level: string;
}

export interface StreamEntry {
  forwarder_id: string;
  reader_ip: string;
  subscribed: boolean;
  local_port: number | null;
  online?: boolean;
  display_alias?: string;
}

export interface StreamsResponse {
  streams: StreamEntry[];
  degraded: boolean;
  upstream_error: string | null;
}

export interface SubscriptionItem {
  forwarder_id: string;
  reader_ip: string;
  local_port_override: number | null;
}

export type ConnectionState =
  | "disconnected"
  | "connecting"
  | "connected"
  | "disconnecting";

export interface StatusResponse {
  connection_state: ConnectionState;
  local_ok: boolean;
  streams_count: number;
}

export interface LogsResponse {
  entries: string[];
}

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

export async function getProfile(): Promise<Profile> {
  return apiFetch<Profile>("/api/v1/profile");
}

export async function putProfile(profile: Profile): Promise<void> {
  await apiFetch("/api/v1/profile", {
    method: "PUT",
    body: JSON.stringify(profile),
  });
}

export async function getStreams(): Promise<StreamsResponse> {
  return apiFetch<StreamsResponse>("/api/v1/streams");
}

export async function putSubscriptions(
  subscriptions: SubscriptionItem[],
): Promise<void> {
  await apiFetch("/api/v1/subscriptions", {
    method: "PUT",
    body: JSON.stringify({ subscriptions }),
  });
}

export async function getStatus(): Promise<StatusResponse> {
  return apiFetch<StatusResponse>("/api/v1/status");
}

export async function getLogs(): Promise<LogsResponse> {
  return apiFetch<LogsResponse>("/api/v1/logs");
}

export async function connect(): Promise<void> {
  const resp = await fetch(`${BASE}/api/v1/connect`, { method: "POST" });
  if (resp.status !== 200 && resp.status !== 202)
    throw new Error(`connect -> ${resp.status}`);
}

export async function disconnect(): Promise<void> {
  const resp = await fetch(`${BASE}/api/v1/disconnect`, { method: "POST" });
  if (resp.status !== 200 && resp.status !== 202)
    throw new Error(`disconnect -> ${resp.status}`);
}

export interface UpdateStatusResponse {
  status: "up_to_date" | "available" | "downloaded" | "failed";
  version?: string;
  error?: string;
}

export async function getUpdateStatus(): Promise<UpdateStatusResponse> {
  return apiFetch<UpdateStatusResponse>("/api/v1/update/status");
}

export async function applyUpdate(): Promise<void> {
  const resp = await fetch(`${BASE}/api/v1/update/apply`, { method: "POST" });
  if (resp.status !== 200) throw new Error(`apply update -> ${resp.status}`);
}
