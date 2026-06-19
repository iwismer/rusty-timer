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
  // Canonical P2P stream identity (always present).
  forwarder_endpoint_id: string;
  stream_id: string;
  // Optional display metadata, populated when the backend has it from the
  // forwarder's P2P catalog.
  forwarder_id?: string | null;
  reader_ip?: string | null;
  subscribed: boolean;
  local_port: number | null;
  event_type?: "start" | "finish";
  online?: boolean | null;
  reader_connected?: boolean | null;
  display_alias?: string | null;
  stream_epoch?: number | null;
  current_epoch_name?: string | null;
  reads_total?: number | null;
  reads_epoch?: number | null;
  cursor_epoch?: number | null;
  cursor_seq?: number | null;
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
  forwarder_endpoint_id: string;
  stream_id: string;
  local_port_override: number | null;
  event_type?: "start" | "finish";
  // Optional legacy display metadata; the backend may echo it back.
  forwarder_id?: string;
  reader_ip?: string;
}

/** Canonical control-API stream identity. */
export interface StreamIdentity {
  forwarder_endpoint_id: string;
  stream_id: string;
}

/** Canonical earliest-epoch override request (control API). */
export interface EarliestEpochRequest {
  forwarder_endpoint_id: string;
  stream_id: string;
  earliest_epoch: number;
}

export type ConnectionState =
  | "disconnected"
  | "connecting"
  | "connected"
  | "disconnecting";

export interface ServerDeviceStatus {
  configured: boolean;
  endpoint_id: string | null;
  reachable: boolean | null;
  approval_state: string | null;
  waiting_for_approval: boolean;
  message: string | null;
}

export type ForwarderConnState =
  | "subscribed"
  | "connected"
  | "unavailable"
  | "disconnected";

export interface ForwarderConnectionStatus {
  endpoint_id: string;
  display_name: string | null;
  state: ForwarderConnState;
  pending: boolean;
  subscribed_count: number;
  available_count: number;
  readers: unknown[];
  ups: ForwarderUpsState | null;
  restart_needed: boolean | null;
}

export interface ConnectionsResponse {
  server: ServerDeviceStatus;
  forwarders: ForwarderConnectionStatus[];
}

export interface StatusResponse {
  connection_state: ConnectionState;
  local_ok: boolean;
  streams_count: number;
  receiver_id: string;
  server: ServerDeviceStatus;
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

export interface ForwarderMetricsUpdate {
  forwarder_id: string;
  unique_chips: number;
  total_reads: number;
  last_read_at: string | null;
}

export interface UpsStatus {
  battery_percent: number;
  battery_voltage_mv: number;
  charging: boolean;
  power_plugged: boolean;
  temperature_cdeg: number;
  sampled_at: number;
}

export interface ForwarderUpsState {
  available: boolean;
  status: UpsStatus | null;
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

export async function getStreamMetrics(): Promise<StreamMetrics[]> {
  return invoke<StreamMetrics[]>("get_stream_metrics");
}

export async function putSubscriptions(
  subscriptions: SubscriptionItem[],
): Promise<void> {
  await invoke("put_subscriptions", { body: { subscriptions } });
}

export async function getStatus(): Promise<StatusResponse> {
  return invoke<StatusResponse>("get_status");
}

export async function reconnectServer(): Promise<void> {
  await invoke("reconnect_server");
}

export async function getConnections(): Promise<ConnectionsResponse> {
  return invoke<ConnectionsResponse>("get_connections");
}

export async function connectForwarder(endpointId: string): Promise<void> {
  await invoke("connect_forwarder", { endpointId });
}

export async function disconnectForwarder(endpointId: string): Promise<void> {
  await invoke("disconnect_forwarder", { endpointId });
}

export async function reconnectForwarder(endpointId: string): Promise<void> {
  await invoke("reconnect_forwarder", { endpointId });
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
  body: EarliestEpochRequest,
): Promise<void> {
  await invoke("put_earliest_epoch", { body });
}

export async function getReplayTargetEpochs(
  stream: StreamRef,
): Promise<ReplayTargetEpochsResponse> {
  return invoke<ReplayTargetEpochsResponse>("get_replay_target_epochs", {
    forwarderId: stream.forwarder_id,
    readerIp: stream.reader_ip,
  });
}

export async function resetStreamCursor(stream: {
  stream_id: string;
}): Promise<void> {
  await invoke("admin_reset_cursor", { body: { stream_id: stream.stream_id } });
}

export async function resetAllCursors(): Promise<{ deleted: number }> {
  return invoke("admin_reset_all_cursors");
}

export async function resetEarliestEpoch(stream: {
  stream_id: string;
}): Promise<void> {
  await invoke("admin_reset_earliest_epoch", {
    body: { stream_id: stream.stream_id },
  });
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

export async function clearData(): Promise<void> {
  await invoke("admin_clear_data");
}

export async function factoryReset(): Promise<void> {
  await invoke("admin_factory_reset");
}

export async function updateLocalPort(
  stream: StreamIdentity,
  localPortOverride: number | null,
): Promise<void> {
  await invoke("admin_update_port", {
    body: {
      forwarder_endpoint_id: stream.forwarder_endpoint_id,
      stream_id: stream.stream_id,
      local_port_override: localPortOverride,
    },
  });
}

export async function getSubscriptions(): Promise<{
  subscriptions: SubscriptionItem[];
}> {
  return invoke<{ subscriptions: SubscriptionItem[] }>("get_subscriptions");
}

export interface DbfConfig {
  enabled: boolean;
  path: string;
}

export async function getDbfConfig(): Promise<DbfConfig> {
  return invoke<DbfConfig>("get_dbf_config");
}

export async function putDbfConfig(config: DbfConfig): Promise<void> {
  await invoke("put_dbf_config", { body: config });
}

export async function clearDbf(): Promise<void> {
  await invoke("clear_dbf");
}

export async function updateSubscriptionEventType(
  stream: StreamIdentity,
  eventType: "start" | "finish",
): Promise<void> {
  await invoke("update_subscription_event_type", {
    forwarderEndpointId: stream.forwarder_endpoint_id,
    streamId: stream.stream_id,
    body: { event_type: eventType },
  });
}
