// Shared reactive state for the receiver UI.
// All tabs, toolbar, and status bar read from this module.

import * as api from "./api";
import type {
  LastRead,
  RaceEntry,
  ReceiverMode,
  StatusResponse,
  StreamCountUpdate,
  StreamsResponse,
} from "./api";
import { buildUpdatedSubscriptions } from "./subscriptions";
import { initSSE, destroySSE } from "./sse";
import { cycleTheme } from "@rusty-timer/shared-ui/lib/dark-mode";
import {
  checkForDesktopUpdate,
  installDesktopUpdate,
  loadDesktopVersion,
  type DesktopUpdateInfo,
} from "./desktop-updater";

// --------------- Tab enum ---------------

export type TabId =
  | "streams"
  | "forwarders"
  | "mode"
  | "config"
  | "logs"
  | "admin";

export type UpdateState = {
  status: "available" | "downloaded";
  currentVersion: string;
  version: string;
  notes: string | null;
  busy: boolean;
  error: string | null;
};

// --------------- Reactive state ---------------
// Wrapped in a single object so Svelte 5 allows export + mutation.

export const store = $state({
  // UI
  activeTab: "streams" as TabId,
  showHelpModal: false,
  helpScrollTarget: null as string | null,

  // Connection / status
  status: null as StatusResponse | null,
  error: null as string | null,
  connectBusy: false,

  // Streams
  streams: null as StreamsResponse | null,
  lastReads: new Map<string, LastRead>(),
  streamMetrics: new Map<string, api.StreamMetrics>(),

  // Forwarders
  forwarders: null as api.ForwardersResponse | null,
  forwardersError: null as string | null,
  selectedForwarderId: null as string | null,

  // Logs
  logEntries: [] as string[],

  // Config (edit + saved for dirty detection)
  editServerUrl: "",
  editToken: "",
  editReceiverId: "",
  savedServerUrl: "",
  savedToken: "",
  savedReceiverId: "",
  saving: false,
  checkingUpdate: false,
  checkMessage: null as string | null,

  // Update
  updateModalOpen: false,
  updateState: null as UpdateState | null,

  // Mode
  races: [] as RaceEntry[],
  modeDraft: "live" as ReceiverMode["mode"],
  raceIdDraft: "",
  earliestEpochInputs: {} as Record<string, string>,
  earliestEpochOptions: {} as Record<string, api.ReplayTargetEpochOption[]>,
  earliestEpochLoading: {} as Record<string, boolean>,
  earliestEpochLoadErrors: {} as Record<string, string>,
  earliestEpochSaving: {} as Record<string, boolean>,
  targetedEpochInputs: {} as Record<string, string>,
  modeBusy: false,
  modeApplyQueued: false,
  savedModePayload: null as string | null,
  modeEditedSinceHydration: false,

  // Stream action state
  streamActionBusy: false,

  // Version info
  appVersion: "",
});

// Version tracking counters (stale-write guards) — not reactive, internal only
let modeHydrationVersion = 0;
let modeEditVersion = 0;
let modeMutationVersion = 0;
let streamRefreshVersion = 0;

// Load queue
let loadAllInFlight = false;
let loadAllQueued = false;

// Tauri event listener cleanup
let tauriUnlistenFns: (() => void)[] = [];

// --------------- Derived state ---------------

export function getConfigDirty(): boolean {
  return (
    store.editServerUrl !== store.savedServerUrl ||
    store.editToken !== store.savedToken ||
    store.editReceiverId !== store.savedReceiverId
  );
}

export function getConnectionState(): string {
  return store.status?.connection_state ?? "unknown";
}

export function getConnectionBadgeState(): "ok" | "warn" | "err" {
  const cs = getConnectionState();
  return cs === "connected" ? "ok" : cs === "disconnected" ? "err" : "warn";
}

// --------------- Setters (for components that need to write imported state) ---------------

export function setActiveTab(tab: TabId): void {
  store.activeTab = tab;
}

export function setShowHelpModal(show: boolean): void {
  store.showHelpModal = show;
}

export function setHelpScrollTarget(target: string | null): void {
  store.helpScrollTarget = target;
}

export function openUpdateModal(): void {
  store.updateModalOpen = true;
}

export function closeUpdateModal(): void {
  store.updateModalOpen = false;
}

function setUpdateState(
  update: DesktopUpdateInfo,
  extra: Partial<Pick<UpdateState, "busy" | "error" | "status">> = {},
): void {
  store.updateState = {
    status: extra.status ?? "available",
    currentVersion: update.currentVersion,
    version: update.version,
    notes: update.notes,
    busy: extra.busy ?? false,
    error: extra.error ?? null,
  };
}

export function setEditServerUrl(value: string): void {
  store.editServerUrl = value;
}

export function setEditToken(value: string): void {
  store.editToken = value;
}

export function setEditReceiverId(value: string): void {
  store.editReceiverId = value;
}

export function setModeDraft(value: ReceiverMode["mode"]): void {
  store.modeDraft = value;
}

export function setRaceIdDraft(value: string): void {
  store.raceIdDraft = value;
}

export function setTargetedEpochInputs(value: Record<string, string>): void {
  store.targetedEpochInputs = value;
}

// --------------- Helpers ---------------

export function streamKey(forwarder_id: string, reader_ip: string): string {
  return `${forwarder_id}/${reader_ip}`;
}

export function parseStreamKey(value: string): api.StreamRef | null {
  const separator = value.indexOf("/");
  if (separator <= 0 || separator === value.length - 1) return null;
  const forwarder_id = value.slice(0, separator).trim();
  const reader_ip = value.slice(separator + 1).trim();
  if (!forwarder_id || !reader_ip) return null;
  return { forwarder_id, reader_ip };
}

export function parseNonNegativeInt(raw: unknown): number | null {
  if (typeof raw === "number") {
    return !Number.isSafeInteger(raw) || raw < 0 ? null : raw;
  }
  if (typeof raw !== "string") return null;
  const trimmed = raw.trim();
  if (!/^\d+$/.test(trimmed)) return null;
  const parsed = Number(trimmed);
  return !Number.isSafeInteger(parsed) || parsed < 0 ? null : parsed;
}

export function isApiReturnedEpoch(key: string, epoch: number): boolean {
  return (store.earliestEpochOptions[key] ?? []).some(
    (option) => option.stream_epoch === epoch,
  );
}

export function parseApiReturnedEpoch(
  key: string,
  raw: unknown,
): number | null {
  const parsed = parseNonNegativeInt(raw);
  if (parsed === null) return null;
  return isApiReturnedEpoch(key, parsed) ? parsed : null;
}

export function formatEarliestEpochOption(
  option: api.ReplayTargetEpochOption,
): string {
  const name = option.name?.trim();
  return name && name.length > 0
    ? `${option.stream_epoch} (${name})`
    : String(option.stream_epoch);
}

export function selectedEarliestEpochValue(stream: api.StreamEntry): string {
  const key = streamKey(stream.forwarder_id, stream.reader_ip);
  const configured = store.earliestEpochInputs[key];
  const options = store.earliestEpochOptions[key] ?? [];

  if (
    configured &&
    options.some((option) => String(option.stream_epoch) === configured)
  ) {
    return configured;
  }
  if (options.length === 0) return "";
  if (
    stream.stream_epoch !== undefined &&
    options.some((option) => option.stream_epoch === stream.stream_epoch)
  ) {
    return String(stream.stream_epoch);
  }
  const newest = options.reduce(
    (max, option) => Math.max(max, option.stream_epoch),
    options[0]?.stream_epoch ?? 0,
  );
  return String(newest);
}

export function selectedTargetedEpochValue(stream: api.StreamEntry): string {
  const key = streamKey(stream.forwarder_id, stream.reader_ip);
  const configured = parseApiReturnedEpoch(key, store.targetedEpochInputs[key]);
  const options = store.earliestEpochOptions[key] ?? [];

  if (configured !== null) return String(configured);
  if (options.length === 0) return "";
  if (
    stream.stream_epoch !== undefined &&
    isApiReturnedEpoch(key, stream.stream_epoch)
  ) {
    return String(stream.stream_epoch);
  }
  const newest = options.reduce(
    (max, option) => Math.max(max, option.stream_epoch),
    options[0]?.stream_epoch ?? 0,
  );
  return String(newest);
}

export function resolveReplayTargetEpoch(
  stream: api.StreamEntry,
): number | null {
  const key = streamKey(stream.forwarder_id, stream.reader_ip);
  const configured = parseApiReturnedEpoch(key, store.targetedEpochInputs[key]);
  if (configured !== null) return configured;
  const selected = parseApiReturnedEpoch(
    key,
    selectedTargetedEpochValue(stream),
  );
  if (selected !== null) return selected;
  return parseNonNegativeInt(stream.stream_epoch);
}

function compareStreamRefs(
  left: { forwarder_id: string; reader_ip: string },
  right: { forwarder_id: string; reader_ip: string },
): number {
  const fc = left.forwarder_id.localeCompare(right.forwarder_id);
  return fc !== 0 ? fc : left.reader_ip.localeCompare(right.reader_ip);
}

export function modePayload(): ReceiverMode {
  if (store.modeDraft === "race") {
    return { mode: "race", race_id: store.raceIdDraft.trim() };
  }
  if (store.modeDraft === "targeted_replay") {
    const targets = Object.entries(store.targetedEpochInputs)
      .map(([key, value]) => {
        const stream = parseStreamKey(key);
        const stream_epoch = parseApiReturnedEpoch(key, value);
        if (!stream || stream_epoch === null) return null;
        return {
          forwarder_id: stream.forwarder_id,
          reader_ip: stream.reader_ip,
          stream_epoch,
        };
      })
      .filter((t): t is api.ReplayTarget => t !== null);
    return { mode: "targeted_replay", targets };
  }
  const liveStreams = (store.streams?.streams ?? []).map((s) => ({
    forwarder_id: s.forwarder_id,
    reader_ip: s.reader_ip,
  }));
  const earliest_epochs = Object.entries(store.earliestEpochInputs)
    .map(([key, value]) => {
      const stream = parseStreamKey(key);
      const earliest_epoch = parseNonNegativeInt(value);
      if (!stream || earliest_epoch === null) return null;
      return {
        forwarder_id: stream.forwarder_id,
        reader_ip: stream.reader_ip,
        earliest_epoch,
      };
    })
    .filter(
      (
        r,
      ): r is {
        forwarder_id: string;
        reader_ip: string;
        earliest_epoch: number;
      } => r !== null,
    );
  return { mode: "live", streams: liveStreams, earliest_epochs };
}

export function modeSignature(mode: ReceiverMode): string {
  if (mode.mode === "race") {
    return JSON.stringify({ mode: "race", race_id: mode.race_id.trim() });
  }
  if (mode.mode === "targeted_replay") {
    const targets = [...mode.targets]
      .map((t) => ({
        forwarder_id: t.forwarder_id,
        reader_ip: t.reader_ip,
        stream_epoch: t.stream_epoch,
      }))
      .sort((a, b) => {
        const sc = compareStreamRefs(a, b);
        return sc !== 0 ? sc : a.stream_epoch - b.stream_epoch;
      });
    return JSON.stringify({ mode: "targeted_replay", targets });
  }
  const liveMode = mode as {
    streams?: api.StreamRef[];
    earliest_epochs?: api.EarliestEpochOverride[];
  };
  const sortedStreams = [...(liveMode.streams ?? [])]
    .map((s) => ({
      forwarder_id: s.forwarder_id,
      reader_ip: s.reader_ip,
    }))
    .sort(compareStreamRefs);
  const earliestEpochRows = Array.isArray(liveMode.earliest_epochs)
    ? liveMode.earliest_epochs
    : [];
  const sorted = [...earliestEpochRows]
    .map((r) => ({
      forwarder_id: r.forwarder_id,
      reader_ip: r.reader_ip,
      earliest_epoch: r.earliest_epoch,
    }))
    .sort((a, b) => {
      const sc = compareStreamRefs(a, b);
      return sc !== 0 ? sc : a.earliest_epoch - b.earliest_epoch;
    });
  return JSON.stringify({
    mode: "live",
    streams: sortedStreams,
    earliest_epochs: sorted,
  });
}

export function getModeDirty(): boolean {
  return store.savedModePayload === null
    ? store.modeEditedSinceHydration
    : modeSignature(modePayload()) !== store.savedModePayload;
}

// --------------- Actions ---------------

export async function prefetchEarliestEpochOptions(
  streamList: api.StreamEntry[],
  forceRefreshKeys: Set<string> = new Set(),
): Promise<void> {
  const tasks = streamList.map(async (stream) => {
    const key = streamKey(stream.forwarder_id, stream.reader_ip);
    const forceRefresh = forceRefreshKeys.has(key);
    if (
      (!forceRefresh && store.earliestEpochOptions[key]) ||
      store.earliestEpochLoading[key]
    )
      return;

    store.earliestEpochLoading = { ...store.earliestEpochLoading, [key]: true };
    store.earliestEpochLoadErrors = {
      ...store.earliestEpochLoadErrors,
      [key]: "",
    };

    try {
      const response = await api.getReplayTargetEpochs({
        forwarder_id: stream.forwarder_id,
        reader_ip: stream.reader_ip,
      });
      store.earliestEpochOptions = {
        ...store.earliestEpochOptions,
        [key]: [...response.epochs].sort(
          (a, b) => b.stream_epoch - a.stream_epoch,
        ),
      };
    } catch (e) {
      store.earliestEpochLoadErrors = {
        ...store.earliestEpochLoadErrors,
        [key]: String(e),
      };
    } finally {
      store.earliestEpochLoading = {
        ...store.earliestEpochLoading,
        [key]: false,
      };
    }
  });
  await Promise.allSettled(tasks);
}

function hydrateMode(mode: ReceiverMode): void {
  store.modeDraft = mode.mode;
  if (mode.mode === "live") {
    const rows = Array.isArray(mode.earliest_epochs)
      ? mode.earliest_epochs
      : [];
    store.earliestEpochInputs = Object.fromEntries(
      rows.map((r) => [
        streamKey(r.forwarder_id, r.reader_ip),
        String(r.earliest_epoch),
      ]),
    );
    store.raceIdDraft = "";
    store.targetedEpochInputs = {};
    return;
  }
  if (mode.mode === "race") {
    store.raceIdDraft = mode.race_id;
    store.targetedEpochInputs = {};
    return;
  }
  store.targetedEpochInputs = Object.fromEntries(
    mode.targets.map((t) => [
      streamKey(t.forwarder_id, t.reader_ip),
      String(t.stream_epoch),
    ]),
  );
}

export function applyHydratedMode(mode: ReceiverMode): void {
  hydrateMode(mode);
  store.savedModePayload = modeSignature(mode);
  store.modeEditedSinceHydration = false;
  modeHydrationVersion += 1;
}

export function markModeEdited(): void {
  store.modeEditedSinceHydration = true;
  modeEditVersion += 1;
}

function applyStreamCountUpdates(updates: StreamCountUpdate[]): boolean {
  if (updates.length === 0) return false;
  if (!store.streams) return true;

  const knownKeys = new Set(
    store.streams.streams.map((s) => streamKey(s.forwarder_id, s.reader_ip)),
  );
  const updatesByKey = new Map(
    updates.map((u) => [streamKey(u.forwarder_id, u.reader_ip), u]),
  );
  const hasUnknown = updates.some(
    (u) => !knownKeys.has(streamKey(u.forwarder_id, u.reader_ip)),
  );

  store.streams = {
    ...store.streams,
    streams: store.streams.streams.map((s) => {
      if (!s.subscribed) return s;
      const u = updatesByKey.get(streamKey(s.forwarder_id, s.reader_ip));
      if (!u) return s;
      return { ...s, reads_total: u.reads_total, reads_epoch: u.reads_epoch };
    }),
  };
  return hasUnknown;
}

export async function loadAll(): Promise<void> {
  if (loadAllInFlight) {
    loadAllQueued = true;
    return;
  }
  loadAllInFlight = true;
  try {
    const modeVersionAtStart = modeHydrationVersion;
    const modeEditVersionAtStart = modeEditVersion;
    const modeMutationVersionAtStart = modeMutationVersion;
    const streamRefreshVersionAtStart = streamRefreshVersion;
    const [
      nextStatus,
      nextStreams,
      nextLogs,
      nextMode,
      nextRaces,
      nextForwarders,
    ] = await Promise.all([
      api.getStatus(),
      api.getStreams(),
      api.getLogs(),
      api.getMode().catch(() => null),
      api.getRaces().catch(() => null),
      api
        .getForwarders()
        .then((forwarders) => ({ ok: true as const, forwarders }))
        .catch((error: unknown) => ({
          ok: false as const,
          error: String(error),
        })),
    ]);

    store.status = nextStatus;
    if (streamRefreshVersion === streamRefreshVersionAtStart) {
      store.streams = nextStreams;
      void prefetchEarliestEpochOptions(nextStreams.streams);
    }
    store.logEntries = nextLogs.entries;
    if (nextRaces) {
      const prevRaceId = store.raceIdDraft;
      store.races = nextRaces.races;
      if (
        store.modeDraft === "race" &&
        prevRaceId.length > 0 &&
        store.races.some((r) => r.race_id === prevRaceId)
      ) {
        store.raceIdDraft = prevRaceId;
      }
    }
    if (nextForwarders.ok) {
      store.forwarders = nextForwarders.forwarders;
      store.forwardersError = null;
    } else {
      store.forwardersError = nextForwarders.error;
    }
    if (
      nextMode &&
      !getModeDirty() &&
      modeEditVersion === modeEditVersionAtStart &&
      modeHydrationVersion === modeVersionAtStart &&
      modeMutationVersion === modeMutationVersionAtStart
    ) {
      applyHydratedMode(nextMode);
    }

    const p = await api.getProfile().catch(() => null);
    if (p) {
      const configWasDirty = getConfigDirty();
      store.savedServerUrl = p.server_url;
      store.savedToken = p.token;
      store.savedReceiverId = p.receiver_id;
      // Only overwrite edit fields if the user hasn't made unsaved changes.
      if (!configWasDirty) {
        store.editServerUrl = p.server_url;
        store.editToken = p.token;
        store.editReceiverId = p.receiver_id;
      }
    }
  } catch (e) {
    store.error = String(e);
  } finally {
    loadAllInFlight = false;
    if (loadAllQueued) {
      loadAllQueued = false;
      void loadAll();
    }
  }
}

export async function loadForwarders(): Promise<void> {
  store.forwardersError = null;
  try {
    const result = await api.getForwarders();
    store.forwarders = result;
  } catch (error) {
    store.forwardersError = String(error);
  }
}

export function selectForwarder(forwarderId: string | null): void {
  store.selectedForwarderId = forwarderId;
}

export async function applyMode(): Promise<void> {
  store.modeApplyQueued = true;
  if (store.modeBusy) return;
  store.modeBusy = true;
  store.error = null;

  while (store.modeApplyQueued) {
    store.modeApplyQueued = false;
    const payload = modePayload();
    if (payload.mode === "race" && payload.race_id.length === 0) {
      store.error = "Select a race before applying Race mode.";
      continue;
    }
    try {
      await api.putMode(payload);
      modeMutationVersion += 1;
      store.savedModePayload = modeSignature(payload);
      store.modeEditedSinceHydration = false;
      store.error = null;
    } catch (e) {
      store.error = String(e);
      if (!store.modeApplyQueued) break;
    }
  }
  store.modeBusy = false;
}

export async function changeEarliestEpoch(
  stream: api.StreamEntry,
  rawValue: string,
): Promise<void> {
  if (store.modeDraft === "race") return;
  const key = streamKey(stream.forwarder_id, stream.reader_ip);
  if (store.earliestEpochSaving[key]) return;

  const parsed = parseNonNegativeInt(rawValue);
  if (parsed === null) {
    store.error = "Earliest epoch must be a non-negative integer.";
    return;
  }

  store.earliestEpochSaving = { ...store.earliestEpochSaving, [key]: true };
  try {
    store.error = null;
    await api.putEarliestEpoch({
      forwarder_id: stream.forwarder_id,
      reader_ip: stream.reader_ip,
      earliest_epoch: parsed,
    });
    store.earliestEpochInputs = {
      ...store.earliestEpochInputs,
      [key]: String(parsed),
    };
    markModeEdited();
  } catch (e) {
    store.error = String(e);
  } finally {
    store.earliestEpochSaving = { ...store.earliestEpochSaving, [key]: false };
  }
}

export async function toggleSubscription(
  stream: api.StreamEntry,
): Promise<void> {
  if (store.streamActionBusy || !store.streams) return;
  store.streamActionBusy = true;
  const refreshVersion = ++streamRefreshVersion;
  try {
    store.error = null;
    const result = buildUpdatedSubscriptions({
      allStreams: store.streams.streams,
      target: {
        forwarder_id: stream.forwarder_id,
        reader_ip: stream.reader_ip,
        currentlySubscribed: stream.subscribed,
      },
    });
    if (result.error) {
      store.error = result.error;
      return;
    }
    await api.putSubscriptions(result.subscriptions!);
    const latestStreams = await api.getStreams();
    if (refreshVersion === streamRefreshVersion) {
      store.streams = latestStreams;
      void prefetchEarliestEpochOptions(latestStreams.streams);
    }
  } catch (e) {
    store.error = String(e);
  } finally {
    store.streamActionBusy = false;
  }
}

export async function replayStream(stream: api.StreamEntry): Promise<void> {
  const parsed = resolveReplayTargetEpoch(stream);
  if (parsed === null) {
    store.error = "Select a valid target epoch before replaying.";
    return;
  }
  try {
    store.error = null;
    const payload: ReceiverMode = {
      mode: "targeted_replay",
      targets: [
        {
          forwarder_id: stream.forwarder_id,
          reader_ip: stream.reader_ip,
          stream_epoch: parsed,
        },
      ],
    };
    await api.putMode(payload);
    modeMutationVersion += 1;
    store.modeDraft = "targeted_replay";
    store.savedModePayload = modeSignature(payload);
    store.modeEditedSinceHydration = false;
  } catch (e) {
    store.error = String(e);
  }
}

export async function replayAll(): Promise<void> {
  const targets = (store.streams?.streams ?? [])
    .map((s) => {
      const epoch = resolveReplayTargetEpoch(s);
      if (epoch === null) return null;
      return {
        forwarder_id: s.forwarder_id,
        reader_ip: s.reader_ip,
        stream_epoch: epoch,
      };
    })
    .filter((t): t is api.ReplayTarget => t !== null);

  if (targets.length === 0) {
    store.error =
      "Select at least one valid target epoch before replaying all.";
    return;
  }
  try {
    store.error = null;
    const payload: ReceiverMode = { mode: "targeted_replay", targets };
    await api.putMode(payload);
    modeMutationVersion += 1;
    store.modeDraft = "targeted_replay";
    store.savedModePayload = modeSignature(payload);
    store.modeEditedSinceHydration = false;
  } catch (e) {
    store.error = String(e);
  }
}

export async function saveProfile(): Promise<void> {
  store.saving = true;
  const payload = {
    server_url: store.editServerUrl,
    token: store.editToken,
    receiver_id: store.editReceiverId,
  };
  try {
    await api.putProfile(payload);
    store.savedServerUrl = payload.server_url;
    store.savedToken = payload.token;
    store.savedReceiverId = payload.receiver_id;
  } catch (e) {
    store.error = String(e);
  } finally {
    store.saving = false;
  }
}

export async function handleCheckUpdate(): Promise<void> {
  store.checkingUpdate = true;
  store.checkMessage = null;
  try {
    const result = await checkForDesktopUpdate();
    if (!result.supported) {
      store.checkMessage = "Desktop updates are unavailable in this runtime.";
      return;
    }

    if (!result.update) {
      store.checkMessage = "Up to date.";
      store.updateState = null;
      return;
    }

    setUpdateState(result.update);
    openUpdateModal();
  } catch (e) {
    const message = String(e);
    store.checkMessage = message;
    if (store.updateState) {
      store.updateState = { ...store.updateState, error: message, busy: false };
      openUpdateModal();
    }
  } finally {
    store.checkingUpdate = false;
  }
}

export async function handleConnect(): Promise<void> {
  store.connectBusy = true;
  try {
    await api.connect();
  } catch (e) {
    store.error = String(e);
  } finally {
    store.connectBusy = false;
  }
}

export async function handleDisconnect(): Promise<void> {
  store.connectBusy = true;
  try {
    await api.disconnect();
  } catch (e) {
    store.error = String(e);
  } finally {
    store.connectBusy = false;
  }
}

export async function confirmUpdateInstall(): Promise<void> {
  if (!store.updateState) return;

  store.updateState = { ...store.updateState, busy: true, error: null };
  try {
    await installDesktopUpdate();
  } catch (e) {
    store.updateState = {
      ...store.updateState,
      busy: false,
      error: String(e),
    };
  }
}

// --------------- SSE + Init ---------------

export function initStore(): void {
  void loadAll();

  void loadDesktopVersion()
    .then((versionInfo) => {
      store.appVersion = versionInfo.version ?? "";
    })
    .catch(() => {});

  void checkForDesktopUpdate()
    .then((result) => {
      if (result.supported && result.update) {
        setUpdateState(result.update);
      } else {
        store.updateState = null;
      }
    })
    .catch(() => {
      store.updateState = null;
    });

  // Listen for Tauri native menu events (no-op if not running in Tauri)
  void import("@tauri-apps/api/event")
    .then(async ({ listen }) => {
      const unlistens = await Promise.all([
        listen("menu-check-update", () => void handleCheckUpdate()),
        listen("menu-toggle-theme", () => cycleTheme()),
        listen("menu-open-help", () => {
          setShowHelpModal(true);
        }),
      ]);
      tauriUnlistenFns = unlistens;
    })
    .catch(() => {
      // Not running in Tauri (e.g., dev server in browser) — ignore
    });

  initSSE({
    onStatusChanged: (s) => {
      store.status = s;
      if (s.connection_state === "disconnected") {
        store.streamMetrics = new Map();
      }
    },
    onStreamsSnapshot: (s) => {
      const previousEpochByKey = new Map(
        (store.streams?.streams ?? []).map((st) => [
          streamKey(st.forwarder_id, st.reader_ip),
          st.stream_epoch,
        ]),
      );
      const refreshAllKeys = new Set(
        s.streams.map((st) => streamKey(st.forwarder_id, st.reader_ip)),
      );
      streamRefreshVersion += 1;
      store.streams = s;
      void prefetchEarliestEpochOptions(s.streams, refreshAllKeys);
      // Prune stale metrics
      const currentKeys = new Set(
        s.streams.map((st) => streamKey(st.forwarder_id, st.reader_ip)),
      );
      const prunedMetrics = new Map(store.streamMetrics);
      for (const key of prunedMetrics.keys()) {
        if (!currentKeys.has(key)) prunedMetrics.delete(key);
      }
      for (const stream of s.streams) {
        const key = streamKey(stream.forwarder_id, stream.reader_ip);
        if (
          !previousEpochByKey.has(key) ||
          previousEpochByKey.get(key) !== stream.stream_epoch
        ) {
          prunedMetrics.delete(key);
        }
      }
      store.streamMetrics = prunedMetrics;
    },
    onLogEntry: (entry) => {
      store.logEntries = [entry, ...store.logEntries].slice(0, 500);
    },
    onResync: () => {
      void loadAll();
    },
    onConnectionChange: () => {},
    onStreamCountsUpdated: (updates) => {
      const needsResync = applyStreamCountUpdates(updates);
      if (needsResync) void loadAll();
    },
    onModeChanged: (mode) => {
      applyHydratedMode(mode);
    },
    onLastRead: (read) => {
      const key = streamKey(read.forwarder_id, read.reader_ip);
      const next = new Map(store.lastReads);
      next.set(key, read);
      store.lastReads = next;
    },
    onStreamMetricsUpdated: (metrics) => {
      const key = streamKey(metrics.forwarder_id, metrics.reader_ip);
      const next = new Map(store.streamMetrics);
      next.set(key, metrics);
      store.streamMetrics = next;
    },
  })?.catch((e: unknown) => {
    console.error("initSSE failed:", e);
    store.error = `Event listener initialization failed: ${String(e)}`;
  });
}

export function destroyStore(): void {
  for (const unlisten of tauriUnlistenFns) {
    unlisten();
  }
  tauriUnlistenFns = [];
  destroySSE();
}
