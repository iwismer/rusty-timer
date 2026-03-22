// Receiver UI - Control API client
// All UI-to-receiver communication goes through this module exclusively.
// Uses Tauri IPC invoke() for direct in-process communication.

import { invoke } from "@tauri-apps/api/core";

export interface Profile {
  server_url: string;
  token: string;
  receiver_id: string;
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
  cursor_epoch?: number;
  cursor_seq?: number;
}

export interface StreamCountUpdate {
  forwarder_id: string;
  reader_ip: string;
  reads_total: number;
  reads_epoch: number;
}

export interface LastRead {
  forwarder_id: string;
  reader_ip: string;
  chip_id: string;
  timestamp: string;
  bib?: string | null;
  name?: string | null;
}

export interface StreamMetrics {
  forwarder_id: string;
  reader_ip: string;
  raw_count: number;
  dedup_count: number;
  retransmit_count: number;
  lag_ms: number | null;
  epoch_raw_count: number;
  epoch_dedup_count: number;
  epoch_retransmit_count: number;
  unique_chips: number;
  epoch_last_received_at: string | null;
  epoch_lag_ms: number | null;
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
  receiver_id: string;
}

export interface LogsResponse {
  entries: string[];
}

export interface StreamRef {
  forwarder_id: string;
  reader_ip: string;
}

export interface EarliestEpochOverride {
  forwarder_id: string;
  reader_ip: string;
  earliest_epoch: number;
}

export interface ReplayTarget {
  forwarder_id: string;
  reader_ip: string;
  stream_epoch: number;
  from_seq?: number;
}

export type ReceiverMode =
  | {
      mode: "live";
      streams: StreamRef[];
      earliest_epochs: EarliestEpochOverride[];
    }
  | {
      mode: "race";
      race_id: string;
    }
  | {
      mode: "targeted_replay";
      targets: ReplayTarget[];
    };

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

// --------------- Forwarder types ---------------

export interface ForwarderReaderInfo {
  reader_ip: string;
  connected: boolean;
}

export interface ForwarderEntry {
  forwarder_id: string;
  display_name: string | null;
  online: boolean;
  readers: ForwarderReaderInfo[];
  unique_chips: number;
  total_reads: number;
  last_read_at: string | null;
}

export interface ForwardersResponse {
  forwarders: ForwarderEntry[];
}

export interface ForwarderMetricsUpdate {
  forwarder_id: string;
  unique_chips: number;
  total_reads: number;
  last_read_at: string | null;
}

export interface ForwarderConfigResponse {
  ok: boolean;
  config: Record<string, unknown>;
  restart_needed: boolean;
  error?: string;
}

export interface ForwarderConfigSaveResponse {
  ok: boolean;
  restart_needed: boolean;
  error?: string;
}

export interface ForwarderControlResponse {
  ok: boolean;
  error?: string;
}

// --------------- API functions ---------------

export async function getProfile(): Promise<Profile> {
  return invoke<Profile>("get_profile");
}

export async function putProfile(profile: Profile): Promise<void> {
  await invoke("put_profile", { body: profile });
}

export async function getStreams(): Promise<StreamsResponse> {
  return invoke<StreamsResponse>("get_streams");
}

export async function putSubscriptions(
  subscriptions: SubscriptionItem[],
): Promise<void> {
  await invoke("put_subscriptions", { body: { subscriptions } });
}

export async function getStatus(): Promise<StatusResponse> {
  return invoke<StatusResponse>("get_status");
}

export async function getLogs(): Promise<LogsResponse> {
  return invoke<LogsResponse>("get_logs");
}

export async function getMode(): Promise<ReceiverMode> {
  return invoke<ReceiverMode>("get_mode");
}

export async function putMode(mode: ReceiverMode): Promise<void> {
  await invoke("put_mode", { mode });
}

export async function putEarliestEpoch(
  epochOverride: EarliestEpochOverride,
): Promise<void> {
  await invoke("put_earliest_epoch", { body: epochOverride });
}

export async function getRaces(): Promise<RacesResponse> {
  return invoke<RacesResponse>("get_races");
}

export async function getReplayTargetEpochs(
  stream: StreamRef,
): Promise<ReplayTargetEpochsResponse> {
  return invoke<ReplayTargetEpochsResponse>("get_replay_target_epochs", {
    forwarderId: stream.forwarder_id,
    readerIp: stream.reader_ip,
  });
}

export async function connect(): Promise<void> {
  await invoke("connect");
}

export async function disconnect(): Promise<void> {
  await invoke("disconnect");
}

export async function resetStreamCursor(stream: StreamRef): Promise<void> {
  await invoke("admin_reset_cursor", { body: stream });
}

export async function resetAllCursors(): Promise<{ deleted: number }> {
  return invoke("admin_reset_all_cursors");
}

export async function resetEarliestEpoch(stream: StreamRef): Promise<void> {
  await invoke("admin_reset_earliest_epoch", { body: stream });
}

export async function resetAllEarliestEpochs(): Promise<{ deleted: number }> {
  return invoke("admin_reset_all_earliest_epochs");
}

export async function purgeSubscriptions(): Promise<{ deleted: number }> {
  return invoke("admin_purge_subscriptions");
}

export async function resetProfile(): Promise<void> {
  await invoke("admin_reset_profile");
}

export async function factoryReset(): Promise<void> {
  await invoke("admin_factory_reset");
}

export async function updateLocalPort(
  stream: StreamRef,
  localPortOverride: number | null,
): Promise<void> {
  await invoke("admin_update_port", {
    body: {
      forwarder_id: stream.forwarder_id,
      reader_ip: stream.reader_ip,
      local_port_override: localPortOverride,
    },
  });
}

// --------------- Forwarder commands ---------------

export async function getForwarders(): Promise<ForwardersResponse> {
  return invoke<ForwardersResponse>("get_forwarders");
}

export async function getForwarderConfig(
  forwarderId: string,
): Promise<ForwarderConfigResponse> {
  return invoke<ForwarderConfigResponse>("get_forwarder_config", {
    forwarderId,
  });
}

export async function setForwarderConfig(
  forwarderId: string,
  section: string,
  data: Record<string, unknown>,
): Promise<ForwarderConfigSaveResponse> {
  return invoke<ForwarderConfigSaveResponse>("set_forwarder_config", {
    forwarderId,
    section,
    data,
  });
}

export async function restartForwarderService(
  forwarderId: string,
): Promise<ForwarderControlResponse> {
  return invoke<ForwarderControlResponse>("restart_forwarder_service", {
    forwarderId,
  });
}

export async function restartForwarderDevice(
  forwarderId: string,
): Promise<ForwarderControlResponse> {
  return invoke<ForwarderControlResponse>("restart_forwarder_device", {
    forwarderId,
  });
}

export async function shutdownForwarderDevice(
  forwarderId: string,
): Promise<ForwarderControlResponse> {
  return invoke<ForwarderControlResponse>("shutdown_forwarder_device", {
    forwarderId,
  });
}

export async function getSubscriptions(): Promise<{
  subscriptions: SubscriptionItem[];
}> {
  return invoke<{ subscriptions: SubscriptionItem[] }>("get_subscriptions");
}
