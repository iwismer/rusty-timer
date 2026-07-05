// Receiver UI - Control API client
// All UI-to-receiver communication goes through this module exclusively.
// Uses Tauri IPC invoke() for direct in-process communication.

import { invoke } from "@tauri-apps/api/core";

export interface Profile {
  server_url: string;
  token: string;
  receiver_id: string;
  // Where the effective server config comes from: "env" (environment override
  // active), "profile" (stored profile), or "none". Optional for compatibility.
  server_source?: "env" | "profile" | "none";
  // Global announcer publish toggle state.
  announcer_enabled?: boolean;
  // Receiver-configured cap on visible rows in the server announcer feed.
  announcer_max_list_size?: number;
}

export interface ImportSummary {
  imported: number;
  resolvable_chips: number;
}

// Counts describing imported participant/chip data and how they overlap.
export interface DataStats {
  participants: number;
  chips: number;
  // Participants that have at least one chip assignment.
  matched_participants: number;
  // Participants with no chip assignment.
  participants_without_chips: number;
  // Chip assignments whose bib resolves to a participant.
  resolvable_chips: number;
}

export interface StreamEpochOption {
  stream_epoch: number;
  created_unix_ms?: number | null;
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
  // Stored explicit override, separate from the resolved local_port. Always
  // sent by the backend; optional here so older fixtures/payloads remain valid.
  local_port_override?: number | null;
  // Whether this stream is opted in to announcer publishing. Always sent by the
  // backend; optional here so older fixtures/payloads remain valid.
  announcer_publish?: boolean;
  event_type?: "start" | "finish";
  online?: boolean | null;
  reader_connected?: boolean | null;
  display_alias?: string | null;
  stream_epoch?: number | null;
  epoch_options?: StreamEpochOption[];
  current_epoch_name?: string | null;
  current_epoch_created_unix_ms?: number | null;
  reads_total?: number | null;
  reads_epoch?: number | null;
  cursor_epoch?: number | null;
  cursor_seq?: number | null;
  /** Data task held fail-closed: the earliest-epoch override is unresolvable. */
  override_held?: boolean;
  /** Stored earliest-epoch override for this stream, if any. */
  earliest_epoch?: number | null;
}

export interface StreamCountUpdate {
  forwarder_endpoint_id: string;
  stream_id: string;
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
  /** Composite stream identity (canonical cache key). */
  forwarder_endpoint_id: string;
  stream_id: string;
  /** Display metadata only — may collide across forwarders. */
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

export type ReadMode = "raw" | "event" | "fsls";

export interface ReaderInfo {
  banner?: string | null;
  hardware?: {
    fw_version?: string | null;
    hw_code?: string | null;
    reader_id?: string | null;
  } | null;
  config?: {
    mode: ReadMode;
    timeout: number;
  } | null;
  tto_enabled?: boolean | null;
  clock?: { reader_clock: string; drift_ms: number } | null;
  estimated_stored_reads?: number | null;
  recording?: boolean | null;
  connect_failures?: number;
}

export interface DownloadProgressUpdate {
  reader_ip: string;
  state: "downloading" | "complete" | "error" | "idle";
  stored_reads: number | null;
  downloaded_reads: number;
  progress: number;
  total: number | null;
  last_read_at: string | null;
  error: string | null;
}

export interface ReaderLiveStatus {
  stream_id: string;
  connected: boolean;
  state: string;
  last_read_unix_ms: number | null;
  reads_session?: number | null;
  reads_epoch?: number | null;
  reads_total?: number | null;
  last_seen_secs?: number | null;
  current_epoch?: number | null;
  current_epoch_created_unix_ms?: number | null;
  current_epoch_name?: string | null;
  hardware_reader_id: string | null;
  firmware_version: string | null;
  model: string | null;
  reader_info?: ReaderInfo | null;
  download_progress?: DownloadProgressUpdate | null;
  local_port?: number | null;
}

export interface UpsStatusPayload {
  on_battery: boolean;
  battery_percent: number;
  runtime_seconds: number;
}

export interface ForwarderConnectionStatus {
  endpoint_id: string;
  display_name: string | null;
  state: ForwarderConnState;
  pending: boolean;
  subscribed_count: number;
  available_count: number;
  readers: ReaderLiveStatus[];
  ups: UpsStatusPayload | null;
  /** Wire stream ids whose data subscription failed terminally on the live
   * connection; not retried until reconnect or a subscription config change. */
  failed_stream_ids?: string[];
  restart_needed: boolean | null;
  remote_config_available: boolean;
  reader_control_available?: boolean;
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

export type ReceiverMode =
  | {
      mode: "live";
      streams: StreamRef[];
    }
  | {
      mode: "race";
      race_id: string;
    };

export interface StreamEpochOption {
  stream_epoch: number;
  name: string | null;
  first_seen_at: string | null;
  created_unix_ms?: number | null;
  /** Whether this epoch can be selected as an earliest-epoch override. */
  selectable: boolean;
}

export interface StreamEpochsResponse {
  epochs: StreamEpochOption[];
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

/// Targeted patch for a reader's volatile counters on the Connections tab,
/// pushed by the receiver instead of a full connections reload.
export interface ForwarderReaderCountsUpdate {
  forwarder_id: string;
  stream_id: string;
  reads_session: number;
  reads_epoch: number | null;
  reads_total: number;
  last_read_unix_ms: number | null;
  last_seen_secs: number | null;
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

export async function importParticipants(
  contents: string,
): Promise<ImportSummary> {
  return invoke<ImportSummary>("import_participants", { contents });
}

export async function importChips(contents: string): Promise<ImportSummary> {
  return invoke<ImportSummary>("import_chips", { contents });
}

export async function importParticipantsFile(
  path: string,
): Promise<ImportSummary> {
  return invoke<ImportSummary>("import_participants_file", { path });
}

export async function importChipsFile(path: string): Promise<ImportSummary> {
  return invoke<ImportSummary>("import_chips_file", { path });
}

export interface RdImportConfig {
  enabled: boolean;
  dir: string;
  interval_secs: number;
}

export async function getRdImportConfig(): Promise<RdImportConfig> {
  return invoke<RdImportConfig>("get_rd_import_config");
}

export async function putRdImportConfig(config: RdImportConfig): Promise<void> {
  await invoke("put_rd_import_config", { body: config });
}

export async function getDataStats(): Promise<DataStats> {
  return invoke<DataStats>("get_data_stats", {});
}

export async function setAnnouncerEnabled(enabled: boolean): Promise<void> {
  await invoke("set_announcer_enabled", { enabled });
}

export async function setAnnouncerMaxListSize(
  maxListSize: number,
): Promise<void> {
  await invoke("set_announcer_max_list_size", { maxListSize });
}

export async function setStreamAnnouncerPublish(
  forwarderEndpointId: string,
  streamId: string,
  publish: boolean,
): Promise<void> {
  await invoke("set_stream_announcer_publish", {
    forwarderEndpointId,
    streamId,
    publish,
  });
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

export interface ForwarderConfigResponse {
  config_json: string;
  restart_needed: boolean;
}

export interface SetForwarderConfigResponse {
  ok: boolean;
  restart_needed: boolean;
  error: string | null;
}

export interface RestartForwarderResponse {
  accepted: boolean;
  error: string | null;
}

export interface ReaderControlResult {
  success: boolean;
  message: string;
  reader_info: ReaderInfo | null;
  current_epoch?: number | null;
  current_epoch_created_unix_ms?: number | null;
  current_epoch_name?: string | null;
}

export async function getForwarderConfig(
  endpointId: string,
): Promise<ForwarderConfigResponse> {
  return invoke<ForwarderConfigResponse>("get_forwarder_config", {
    endpointId,
  });
}

export async function setForwarderConfig(
  endpointId: string,
  configJson: string,
): Promise<SetForwarderConfigResponse> {
  return invoke<SetForwarderConfigResponse>("set_forwarder_config", {
    endpointId,
    configJson,
  });
}

export async function restartForwarder(
  endpointId: string,
): Promise<RestartForwarderResponse> {
  return invoke<RestartForwarderResponse>("restart_forwarder", { endpointId });
}

export async function readerGetInfo(
  endpointId: string,
  streamId: string,
): Promise<ReaderControlResult> {
  return invoke<ReaderControlResult>("reader_get_info", {
    endpointId,
    streamId,
  });
}

export async function readerSyncClock(
  endpointId: string,
  streamId: string,
): Promise<ReaderControlResult> {
  return invoke<ReaderControlResult>("reader_sync_clock", {
    endpointId,
    streamId,
  });
}

export async function readerSetEpochName(
  endpointId: string,
  streamId: string,
  name: string | null,
): Promise<ReaderControlResult> {
  return invoke<ReaderControlResult>("reader_set_epoch_name", {
    endpointId,
    streamId,
    name,
  });
}

export async function readerAdvanceEpoch(
  endpointId: string,
  streamId: string,
  name: string | null,
): Promise<ReaderControlResult> {
  return invoke<ReaderControlResult>("reader_advance_epoch", {
    endpointId,
    streamId,
    name,
  });
}

export async function readerSetReadMode(
  endpointId: string,
  streamId: string,
  mode: ReadMode,
  timeout: number,
): Promise<ReaderControlResult> {
  return invoke<ReaderControlResult>("reader_set_read_mode", {
    endpointId,
    streamId,
    mode,
    timeout,
  });
}

export async function readerSetTto(
  endpointId: string,
  streamId: string,
  enabled: boolean,
): Promise<ReaderControlResult> {
  return invoke<ReaderControlResult>("reader_set_tto", {
    endpointId,
    streamId,
    enabled,
  });
}

export async function readerSetRecording(
  endpointId: string,
  streamId: string,
  enabled: boolean,
): Promise<ReaderControlResult> {
  return invoke<ReaderControlResult>("reader_set_recording", {
    endpointId,
    streamId,
    enabled,
  });
}

export async function readerClearRecords(
  endpointId: string,
  streamId: string,
): Promise<ReaderControlResult> {
  return invoke<ReaderControlResult>("reader_clear_records", {
    endpointId,
    streamId,
  });
}

export async function readerStartDownload(
  endpointId: string,
  streamId: string,
): Promise<ReaderControlResult> {
  return invoke<ReaderControlResult>("reader_start_download", {
    endpointId,
    streamId,
  });
}

export async function readerStopDownload(
  endpointId: string,
  streamId: string,
): Promise<ReaderControlResult> {
  return invoke<ReaderControlResult>("reader_stop_download", {
    endpointId,
    streamId,
  });
}

export async function readerRefresh(
  endpointId: string,
  streamId: string,
): Promise<ReaderControlResult> {
  return invoke<ReaderControlResult>("reader_refresh", {
    endpointId,
    streamId,
  });
}

export async function readerReconnect(
  endpointId: string,
  streamId: string,
): Promise<ReaderControlResult> {
  return invoke<ReaderControlResult>("reader_reconnect", {
    endpointId,
    streamId,
  });
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

export async function getStreamEpochs(stream: {
  forwarder_endpoint_id: string;
  stream_id: string;
}): Promise<StreamEpochsResponse> {
  return invoke<StreamEpochsResponse>("get_stream_epochs", {
    forwarderEndpointId: stream.forwarder_endpoint_id,
    streamId: stream.stream_id,
  });
}

export async function resetStreamCursor(stream: {
  forwarder_endpoint_id: string;
  stream_id: string;
}): Promise<void> {
  await invoke("admin_reset_cursor", {
    body: {
      forwarder_endpoint_id: stream.forwarder_endpoint_id,
      stream_id: stream.stream_id,
    },
  });
}

export async function resetStreamData(stream: {
  forwarder_endpoint_id: string;
  stream_id: string;
}): Promise<void> {
  await invoke("admin_reset_stream_data", {
    body: {
      forwarder_endpoint_id: stream.forwarder_endpoint_id,
      stream_id: stream.stream_id,
    },
  });
}

export async function resetAllCursors(): Promise<{ deleted: number }> {
  return invoke("admin_reset_all_cursors");
}

export async function resetEarliestEpoch(stream: {
  forwarder_endpoint_id: string;
  stream_id: string;
}): Promise<void> {
  await invoke("admin_reset_earliest_epoch", {
    body: {
      forwarder_endpoint_id: stream.forwarder_endpoint_id,
      stream_id: stream.stream_id,
    },
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
  /** DBF write coalescing interval in milliseconds (clamped 250–5000). */
  flush_interval_ms: number;
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
