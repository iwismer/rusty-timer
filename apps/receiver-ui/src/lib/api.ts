// Receiver UI - Control API client
// All UI-to-receiver communication goes through this module exclusively.
// Uses same-origin requests (UI is served by the receiver's axum server).

import { apiFetch } from "@rusty-timer/shared-ui/lib/api-helpers";

export interface Profile {
  server_url: string;
  token: string;
  update_mode: string;
}

export interface StreamEntry {
  stream_id?: string;
  forwarder_id: string;
  reader_ip: string;
  subscribed: boolean;
  local_port: number | null;
  online?: boolean;
  display_alias?: string;
  stream_epoch?: number;
  current_epoch_name?: string | null;
  reads_total?: number;
  reads_epoch?: number;
}

export interface StreamCountUpdate {
  forwarder_id: string;
  reader_ip: string;
  reads_total: number;
  reads_epoch: number;
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

export interface StreamRef {
  forwarder_id: string;
  reader_ip: string;
}

export type EpochScope = "all" | "current";

export type ReplayPolicy = "resume" | "live_only" | "targeted";

export type ReceiverSelection =
  | {
      mode: "manual";
      streams: StreamRef[];
    }
  | {
      mode: "race";
      race_id: string;
      epoch_scope: EpochScope;
    };

export interface ReplayTarget {
  forwarder_id: string;
  reader_ip: string;
  stream_epoch: number;
  from_seq?: number;
}

export interface ReceiverSetSelection {
  selection: ReceiverSelection;
  replay_policy: ReplayPolicy;
  replay_targets?: ReplayTarget[];
}

export interface RaceEntry {
  race_id: string;
  name: string;
  created_at: string;
}

export interface RacesResponse {
  races: RaceEntry[];
}

export interface ReplayTargetEpochOption {
  stream_epoch: number;
  name: string | null;
  first_seen_at: string | null;
  race_names: string[];
}

export interface ReplayTargetEpochsResponse {
  epochs: ReplayTargetEpochOption[];
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

export async function getSelection(): Promise<ReceiverSetSelection> {
  return apiFetch<ReceiverSetSelection>("/api/v1/selection");
}

export async function putSelection(
  selection: ReceiverSetSelection,
): Promise<void> {
  await apiFetch("/api/v1/selection", {
    method: "PUT",
    body: JSON.stringify(selection),
  });
}

export async function getRaces(): Promise<RacesResponse> {
  return apiFetch<RacesResponse>("/api/v1/races");
}

export async function getReplayTargetEpochs(
  stream: StreamRef,
): Promise<ReplayTargetEpochsResponse> {
  const params = new URLSearchParams({
    forwarder_id: stream.forwarder_id,
    reader_ip: stream.reader_ip,
  });
  return apiFetch<ReplayTargetEpochsResponse>(
    `/api/v1/replay-targets/epochs?${params.toString()}`,
  );
}

export async function resetStreamCursor(stream: StreamRef): Promise<void> {
  await apiFetch("/api/v1/admin/cursors/reset", {
    method: "POST",
    headers: {
      "x-rt-receiver-admin-intent": "reset-stream-cursor",
    },
    body: JSON.stringify(stream),
  });
}

export async function connect(): Promise<void> {
  const resp = await fetch("/api/v1/connect", { method: "POST" });
  if (resp.status !== 200 && resp.status !== 202)
    throw new Error(`connect -> ${resp.status}`);
}

export async function disconnect(): Promise<void> {
  const resp = await fetch("/api/v1/disconnect", { method: "POST" });
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
  const resp = await fetch("/api/v1/update/apply", { method: "POST" });
  if (resp.status !== 200) throw new Error(`apply update -> ${resp.status}`);
}

export async function checkForUpdate(): Promise<UpdateStatusResponse> {
  return apiFetch<UpdateStatusResponse>("/api/v1/update/check", {
    method: "POST",
  });
}

export async function downloadUpdate(): Promise<UpdateStatusResponse> {
  const resp = await fetch("/api/v1/update/download", { method: "POST" });
  if (resp.status !== 200 && resp.status !== 409) {
    const text = await resp.text();
    throw new Error(`download update -> ${resp.status}: ${text}`);
  }
  return (await resp.json()) as UpdateStatusResponse;
}
