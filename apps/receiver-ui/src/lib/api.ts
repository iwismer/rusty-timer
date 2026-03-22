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
  event_type?: "start" | "finish";
  online?: boolean;
  reader_connected?: boolean;
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
  event_type?: "start" | "finish";
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

/** Corresponds to RaceInfo in rt-protocol. */
export interface RaceEntry {
  race_id: string;
  name: string;
  created_at: string;
  participant_count: number;
  chip_count: number;
}

export interface RacesResponse {
  races: RaceEntry[];
}

export interface ParticipantEntry {
  bib: number;
  first_name: string;
  last_name: string;
  gender: string;
  affiliation: string | null;
  chip_ids: string[];
}

export interface UnmatchedChip {
  chip_id: string;
  bib: number;
}

export interface ParticipantsResponse {
  participants: ParticipantEntry[];
  chips_without_participant: UnmatchedChip[];
}

export interface UploadResult {
  imported: number;
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

export async function createRace(name: string): Promise<RaceEntry> {
  return invoke<RaceEntry>("create_race", { name });
}

export async function deleteRace(raceId: string): Promise<void> {
  await invoke("delete_race", { raceId });
}

export async function getParticipants(
  raceId: string,
): Promise<ParticipantsResponse> {
  return invoke<ParticipantsResponse>("get_participants", { raceId });
}

export async function uploadRaceFile(
  raceId: string,
  uploadType: "participants" | "chips",
  fileData: string,
  fileName: string,
): Promise<UploadResult> {
  return invoke<UploadResult>("upload_race_file", {
    raceId,
    uploadType,
    fileData,
    fileName,
  });
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

export interface ForwarderRaceResponse {
  forwarder_id: string;
  race_id: string | null;
}

export async function getForwarderRace(
  forwarderId: string,
): Promise<ForwarderRaceResponse> {
  return invoke<ForwarderRaceResponse>("get_forwarder_race", { forwarderId });
}

export async function setForwarderRace(
  forwarderId: string,
  raceId: string | null,
): Promise<ForwarderRaceResponse> {
  return invoke<ForwarderRaceResponse>("set_forwarder_race", {
    forwarderId,
    raceId,
  });
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
  stream: StreamRef,
  eventType: "start" | "finish",
): Promise<void> {
  await invoke("update_subscription_event_type", {
    forwarder_id: stream.forwarder_id,
    reader_ip: stream.reader_ip,
    body: { event_type: eventType },
  });
}

// --------------- Announcer commands ---------------

import type {
  AnnouncerConfig,
  AnnouncerConfigUpdate,
  AnnouncerStreamEntry,
} from "@rusty-timer/shared-ui";
export type { AnnouncerConfig, AnnouncerConfigUpdate };

export interface ServerStreamsResponse {
  streams: AnnouncerStreamEntry[];
}

export async function getServerStreams(): Promise<ServerStreamsResponse> {
  return invoke<ServerStreamsResponse>("get_server_streams");
}

export async function getAnnouncerConfig(): Promise<AnnouncerConfig> {
  return invoke<AnnouncerConfig>("get_announcer_config");
}

export async function putAnnouncerConfig(
  body: AnnouncerConfigUpdate,
): Promise<AnnouncerConfig> {
  return invoke<AnnouncerConfig>("put_announcer_config", { body });
}

export async function resetAnnouncer(): Promise<void> {
  await invoke("reset_announcer");
}

// --------------- Reader control types ---------------

export type ReaderConnectionState = "connected" | "connecting" | "disconnected";
export type DownloadState = "downloading" | "complete" | "error" | "idle";

export interface HardwareInfo {
  fw_version?: string | null;
  hw_code?: string | null;
  reader_id?: string | null;
}

export interface Config3Info {
  mode: string;
  timeout: number;
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

// --------------- Reader control commands ---------------

export interface ReaderCommandResponse {
  ok: boolean;
  error?: string;
  reader_info?: ReaderInfo | null;
}

export interface ReaderSimpleResponse {
  ok: boolean;
  error?: string;
}

export async function readerGetInfo(
  forwarderId: string,
  readerIp: string,
): Promise<ReaderCommandResponse> {
  return invoke<ReaderCommandResponse>("reader_get_info", {
    forwarderId,
    readerIp,
  });
}

export async function readerSyncClock(
  forwarderId: string,
  readerIp: string,
): Promise<ReaderCommandResponse> {
  return invoke<ReaderCommandResponse>("reader_sync_clock", {
    forwarderId,
    readerIp,
  });
}

export async function readerSetReadMode(
  forwarderId: string,
  readerIp: string,
  mode: string,
  timeout: number,
): Promise<ReaderCommandResponse> {
  return invoke<ReaderCommandResponse>("reader_set_read_mode", {
    forwarderId,
    readerIp,
    mode,
    timeout,
  });
}

export async function readerSetTto(
  forwarderId: string,
  readerIp: string,
  enabled: boolean,
): Promise<ReaderCommandResponse> {
  return invoke<ReaderCommandResponse>("reader_set_tto", {
    forwarderId,
    readerIp,
    enabled,
  });
}

export async function readerSetRecording(
  forwarderId: string,
  readerIp: string,
  enabled: boolean,
): Promise<ReaderCommandResponse> {
  return invoke<ReaderCommandResponse>("reader_set_recording", {
    forwarderId,
    readerIp,
    enabled,
  });
}

export async function readerClearRecords(
  forwarderId: string,
  readerIp: string,
): Promise<ReaderSimpleResponse> {
  return invoke<ReaderSimpleResponse>("reader_clear_records", {
    forwarderId,
    readerIp,
  });
}

export async function readerStartDownload(
  forwarderId: string,
  readerIp: string,
): Promise<ReaderSimpleResponse> {
  return invoke<ReaderSimpleResponse>("reader_start_download", {
    forwarderId,
    readerIp,
  });
}

export async function readerStopDownload(
  forwarderId: string,
  readerIp: string,
): Promise<ReaderSimpleResponse> {
  return invoke<ReaderSimpleResponse>("reader_stop_download", {
    forwarderId,
    readerIp,
  });
}

export async function readerRefresh(
  forwarderId: string,
  readerIp: string,
): Promise<ReaderCommandResponse> {
  return invoke<ReaderCommandResponse>("reader_refresh", {
    forwarderId,
    readerIp,
  });
}

export async function readerReconnect(
  forwarderId: string,
  readerIp: string,
): Promise<ReaderSimpleResponse> {
  return invoke<ReaderSimpleResponse>("reader_reconnect", {
    forwarderId,
    readerIp,
  });
}
