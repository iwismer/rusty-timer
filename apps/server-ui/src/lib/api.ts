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
  last_tag_id: string | null;
  last_reader_timestamp: string | null;
}

export interface EpochInfo {
  epoch: number;
  event_count: number;
  first_event_at: string | null;
  last_event_at: string | null;
  name: string | null;
  is_current: boolean;
}

export interface ApiError {
  code: string;
  message: string;
  details?: Record<string, unknown>;
}

export interface AnnouncerConfig {
  enabled: boolean;
  enabled_until: string | null;
  selected_stream_ids: string[];
  max_list_size: number;
  updated_at: string;
  public_enabled: boolean;
}

export interface AnnouncerConfigUpdate {
  enabled: boolean;
  selected_stream_ids: string[];
  max_list_size: number;
}

export interface AnnouncerRow {
  stream_id: string;
  seq: number;
  chip_id: string;
  bib: number | null;
  display_name: string;
  reader_timestamp: string | null;
  received_at: string;
}

export interface AnnouncerState extends AnnouncerConfig {
  finisher_count: number;
  rows: AnnouncerRow[];
}

export interface AnnouncerDelta {
  row: AnnouncerRow;
  finisher_count: number;
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

/** GET /api/v1/logs */
export interface LogsResponse {
  entries: string[];
}

export async function getLogs(): Promise<LogsResponse> {
  return apiFetch<LogsResponse>("/api/v1/logs");
}

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
    last_tag_id: (data.last_tag_id as string | null) ?? null,
    last_reader_timestamp:
      (data.last_reader_timestamp as string | null) ?? null,
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

/** Returns the href for the per-epoch export.csv download (no fetch needed — direct link). */
export function epochExportCsvUrl(streamId: string, epoch: number): string {
  return `${BASE}/api/v1/streams/${streamId}/epochs/${epoch}/export.csv`;
}

/** Returns the href for the per-epoch export.txt download (no fetch needed — direct link). */
export function epochExportTxtUrl(streamId: string, epoch: number): string {
  return `${BASE}/api/v1/streams/${streamId}/epochs/${epoch}/export.txt`;
}

/** POST /api/v1/streams/{stream_id}/reset-epoch
 *  Resolves on 204. Throws on 404, 409, or 5xx. */
export async function resetEpoch(streamId: string): Promise<void> {
  return apiFetch<void>(`/api/v1/streams/${streamId}/reset-epoch`, {
    method: "POST",
  });
}

/** GET /api/v1/streams/{stream_id}/epochs — list epochs with metadata */
export async function getStreamEpochs(streamId: string): Promise<EpochInfo[]> {
  return apiFetch<EpochInfo[]>(`/api/v1/streams/${streamId}/epochs`);
}

/** GET /api/v1/announcer/config */
export async function getAnnouncerConfig(): Promise<AnnouncerConfig> {
  return apiFetch<AnnouncerConfig>("/api/v1/announcer/config");
}

/** PUT /api/v1/announcer/config */
export async function updateAnnouncerConfig(
  payload: AnnouncerConfigUpdate,
): Promise<AnnouncerConfig> {
  return apiFetch<AnnouncerConfig>("/api/v1/announcer/config", {
    method: "PUT",
    body: JSON.stringify(payload),
  });
}

/** POST /api/v1/announcer/reset */
export async function resetAnnouncer(): Promise<void> {
  return apiFetch<void>("/api/v1/announcer/reset", {
    method: "POST",
  });
}

/** GET /api/v1/announcer/state */
export async function getAnnouncerState(): Promise<AnnouncerState> {
  return apiFetch<AnnouncerState>("/api/v1/announcer/state");
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

export type ForwarderControlAction =
  | "restart-service"
  | "restart-device"
  | "shutdown-device";

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

/** POST /api/v1/forwarders/{forwarderId}/control/{action} */
export async function controlForwarderAction(
  forwarderId: string,
  action: ForwarderControlAction,
): Promise<{ ok: boolean; error?: string }> {
  const result = await apiFetch<{ ok: boolean; error?: string | null }>(
    `/api/v1/forwarders/${encodeURIComponent(forwarderId)}/control/${encodeURIComponent(action)}`,
    { method: "POST" },
  );
  return {
    ok: result.ok,
    error: result.error ?? undefined,
  };
}

export async function restartForwarderService(
  forwarderId: string,
): Promise<{ ok: boolean; error?: string }> {
  return controlForwarderAction(forwarderId, "restart-service");
}

export async function restartForwarderDevice(
  forwarderId: string,
): Promise<{ ok: boolean; error?: string }> {
  return controlForwarderAction(forwarderId, "restart-device");
}

export async function shutdownForwarderDevice(
  forwarderId: string,
): Promise<{ ok: boolean; error?: string }> {
  return controlForwarderAction(forwarderId, "shutdown-device");
}

// ----- Forwarder-race types -----

export interface ForwarderRaceAssignment {
  forwarder_id: string;
  race_id: string | null;
}

export interface ForwarderRacesResponse {
  assignments: ForwarderRaceAssignment[];
}

// ----- Forwarder-race API -----

/** GET /api/v1/forwarder-races */
export async function getForwarderRaces(): Promise<ForwarderRacesResponse> {
  return apiFetch<ForwarderRacesResponse>("/api/v1/forwarder-races");
}

/** PUT /api/v1/forwarders/{forwarderId}/race */
export async function setForwarderRace(
  forwarderId: string,
  raceId: string | null,
): Promise<void> {
  return apiFetch<void>(
    `/api/v1/forwarders/${encodeURIComponent(forwarderId)}/race`,
    {
      method: "PUT",
      body: JSON.stringify({ race_id: raceId }),
    },
  );
}

// ----- Reads types -----

export type DedupMode = "none" | "first" | "last";
export type SortOrder = "asc" | "desc";

export interface ReadEntry {
  stream_id: string;
  seq: number;
  reader_timestamp: string | null;
  tag_id: string | null;
  received_at: string;
  bib: number | null;
  first_name: string | null;
  last_name: string | null;
}

export interface ReadsResponse {
  reads: ReadEntry[];
  total: number;
  limit: number;
  offset: number;
}

export interface ReadsParams {
  dedup?: DedupMode;
  window_secs?: number;
  limit?: number;
  offset?: number;
  order?: SortOrder;
}

// ----- Reads API -----

function buildReadsQuery(params?: ReadsParams): string {
  if (!params) return "";
  const parts: string[] = [];
  if (params.dedup) parts.push(`dedup=${params.dedup}`);
  if (params.window_secs != null)
    parts.push(`window_secs=${params.window_secs}`);
  if (params.limit != null) parts.push(`limit=${params.limit}`);
  if (params.offset != null) parts.push(`offset=${params.offset}`);
  if (params.order) parts.push(`order=${params.order}`);
  return parts.length ? `?${parts.join("&")}` : "";
}

/** GET /api/v1/streams/{streamId}/reads */
export async function getStreamReads(
  streamId: string,
  params?: ReadsParams,
): Promise<ReadsResponse> {
  return apiFetch<ReadsResponse>(
    `/api/v1/streams/${encodeURIComponent(streamId)}/reads${buildReadsQuery(params)}`,
  );
}

/** GET /api/v1/forwarders/{forwarderId}/reads */
export async function getForwarderReads(
  forwarderId: string,
  params?: ReadsParams,
): Promise<ReadsResponse> {
  return apiFetch<ReadsResponse>(
    `/api/v1/forwarders/${encodeURIComponent(forwarderId)}/reads${buildReadsQuery(params)}`,
  );
}

// ----- Admin types -----

export interface TokenEntry {
  token_id: string;
  device_type: string;
  device_id: string;
  created_at: string;
  revoked: boolean;
}

export interface TokensResponse {
  tokens: TokenEntry[];
}

export interface CreateTokenRequest {
  device_id: string;
  device_type: "forwarder" | "receiver";
  token?: string;
}

export interface CreateTokenResponse {
  token_id: string;
  device_id: string;
  device_type: string;
  token: string;
}

export interface CursorEntry {
  receiver_id: string;
  stream_id: string;
  stream_epoch: number;
  last_seq: number;
  updated_at: string;
}

export interface CursorsResponse {
  cursors: CursorEntry[];
}

// ----- Admin API -----

/** GET /api/v1/admin/tokens */
export async function getTokens(): Promise<TokensResponse> {
  return apiFetch<TokensResponse>("/api/v1/admin/tokens");
}

/** POST /api/v1/admin/tokens/{tokenId}/revoke — returns 204 */
export async function revokeToken(tokenId: string): Promise<void> {
  return apiFetch<void>(`/api/v1/admin/tokens/${tokenId}/revoke`, {
    method: "POST",
  });
}

/** POST /api/v1/admin/tokens — create a new device token */
export async function createToken(
  req: CreateTokenRequest,
): Promise<CreateTokenResponse> {
  return apiFetch<CreateTokenResponse>("/api/v1/admin/tokens", {
    method: "POST",
    body: JSON.stringify(req),
  });
}

/** DELETE /api/v1/admin/tokens — delete ALL device tokens, returns 204 */
export async function deleteAllTokens(): Promise<void> {
  return apiFetch<void>("/api/v1/admin/tokens", { method: "DELETE" });
}

/** DELETE /api/v1/admin/streams/{streamId} — cascade delete, returns 204 */
export async function deleteStream(streamId: string): Promise<void> {
  return apiFetch<void>(`/api/v1/admin/streams/${streamId}`, {
    method: "DELETE",
  });
}

/** DELETE /api/v1/admin/streams — delete ALL streams, returns 204 */
export async function deleteAllStreams(): Promise<void> {
  return apiFetch<void>("/api/v1/admin/streams", { method: "DELETE" });
}

/** DELETE /api/v1/admin/events — clear ALL events, returns 204 */
export async function deleteAllEvents(): Promise<void> {
  return apiFetch<void>("/api/v1/admin/events", { method: "DELETE" });
}

/** DELETE /api/v1/admin/streams/{streamId}/events — clear events for a stream, returns 204 */
export async function deleteStreamEvents(streamId: string): Promise<void> {
  return apiFetch<void>(`/api/v1/admin/streams/${streamId}/events`, {
    method: "DELETE",
  });
}

/** DELETE /api/v1/admin/streams/{streamId}/epochs/{epoch}/events — clear events for an epoch, returns 204 */
export async function deleteEpochEvents(
  streamId: string,
  epoch: number,
): Promise<void> {
  return apiFetch<void>(
    `/api/v1/admin/streams/${streamId}/epochs/${epoch}/events`,
    { method: "DELETE" },
  );
}

/** DELETE /api/v1/admin/receiver-cursors — clear ALL receiver cursors, returns 204 */
export async function deleteAllCursors(): Promise<void> {
  return apiFetch<void>("/api/v1/admin/receiver-cursors", {
    method: "DELETE",
  });
}

/** GET /api/v1/admin/receiver-cursors — list all receiver cursors */
export async function getCursors(): Promise<CursorsResponse> {
  return apiFetch<CursorsResponse>("/api/v1/admin/receiver-cursors");
}

/** DELETE /api/v1/admin/receiver-cursors/{receiverId} — clear cursors for one receiver, returns 204 */
export async function deleteReceiverCursors(receiverId: string): Promise<void> {
  return apiFetch<void>(
    `/api/v1/admin/receiver-cursors/${encodeURIComponent(receiverId)}`,
    { method: "DELETE" },
  );
}

/** DELETE /api/v1/admin/receiver-cursors/{receiverId}/{streamId} — clear a single cursor, returns 204 */
export async function deleteReceiverStreamCursor(
  receiverId: string,
  streamId: string,
): Promise<void> {
  return apiFetch<void>(
    `/api/v1/admin/receiver-cursors/${encodeURIComponent(receiverId)}/${encodeURIComponent(streamId)}`,
    { method: "DELETE" },
  );
}

/** DELETE /api/v1/admin/races — delete ALL races, returns 204 */
export async function deleteAllRaces(): Promise<void> {
  return apiFetch<void>("/api/v1/admin/races", { method: "DELETE" });
}

// ----- Race types -----

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

export interface StreamEpochRaceMapping {
  stream_id: string;
  forwarder_id: string;
  reader_ip: string;
  stream_epoch: number;
  race_id: string | null;
}

export interface RaceStreamEpochMappingsResponse {
  mappings: StreamEpochRaceMapping[];
}

// ----- Race API -----

/** GET /api/v1/races */
export async function getRaces(): Promise<RacesResponse> {
  return apiFetch<RacesResponse>("/api/v1/races");
}

/** POST /api/v1/races */
export async function createRace(name: string): Promise<RaceEntry> {
  return apiFetch<RaceEntry>("/api/v1/races", {
    method: "POST",
    body: JSON.stringify({ name }),
  });
}

/** DELETE /api/v1/races/{raceId} */
export async function deleteRace(raceId: string): Promise<void> {
  return apiFetch<void>(`/api/v1/races/${encodeURIComponent(raceId)}`, {
    method: "DELETE",
  });
}

/** PUT /api/v1/streams/{streamId}/epochs/{epoch}/race */
export async function setStreamEpochRace(
  streamId: string,
  epoch: number,
  raceId: string | null,
): Promise<{
  stream_id: string;
  stream_epoch: number;
  race_id: string | null;
}> {
  return apiFetch<{
    stream_id: string;
    stream_epoch: number;
    race_id: string | null;
  }>(`/api/v1/streams/${encodeURIComponent(streamId)}/epochs/${epoch}/race`, {
    method: "PUT",
    body: JSON.stringify({ race_id: raceId }),
  });
}

/** PUT /api/v1/streams/{streamId}/epochs/{epoch}/name */
export async function setStreamEpochName(
  streamId: string,
  epoch: number,
  name: string | null,
): Promise<{
  stream_id: string;
  stream_epoch: number;
  name: string | null;
}> {
  return apiFetch<{
    stream_id: string;
    stream_epoch: number;
    name: string | null;
  }>(`/api/v1/streams/${encodeURIComponent(streamId)}/epochs/${epoch}/name`, {
    method: "PUT",
    body: JSON.stringify({ name }),
  });
}

/** GET /api/v1/races/{raceId}/stream-epochs */
export async function getRaceStreamEpochMappings(
  raceId: string,
): Promise<RaceStreamEpochMappingsResponse> {
  return apiFetch<RaceStreamEpochMappingsResponse>(
    `/api/v1/races/${encodeURIComponent(raceId)}/stream-epochs`,
  );
}

/** POST /api/v1/races/{raceId}/streams/{streamId}/epochs/activate-next */
export async function activateNextStreamEpochForRace(
  raceId: string,
  streamId: string,
): Promise<void> {
  return apiFetch<void>(
    `/api/v1/races/${encodeURIComponent(raceId)}/streams/${encodeURIComponent(streamId)}/epochs/activate-next`,
    { method: "POST" },
  );
}

/** GET /api/v1/races/{raceId}/participants */
export async function getParticipants(
  raceId: string,
): Promise<ParticipantsResponse> {
  return apiFetch<ParticipantsResponse>(
    `/api/v1/races/${encodeURIComponent(raceId)}/participants`,
  );
}

/** POST /api/v1/races/{raceId}/participants/upload (multipart file) */
export async function uploadParticipants(
  raceId: string,
  file: File,
): Promise<UploadResult> {
  const form = new FormData();
  form.append("file", file);
  const resp = await fetch(
    `${BASE}/api/v1/races/${encodeURIComponent(raceId)}/participants/upload`,
    { method: "POST", body: form },
  );
  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(`Upload failed: ${resp.status}: ${text}`);
  }
  return resp.json();
}

/** POST /api/v1/races/{raceId}/chips/upload (multipart file) */
export async function uploadChips(
  raceId: string,
  file: File,
): Promise<UploadResult> {
  const form = new FormData();
  form.append("file", file);
  const resp = await fetch(
    `${BASE}/api/v1/races/${encodeURIComponent(raceId)}/chips/upload`,
    {
      method: "POST",
      body: form,
    },
  );
  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(`Upload failed: ${resp.status}: ${text}`);
  }
  return resp.json();
}
