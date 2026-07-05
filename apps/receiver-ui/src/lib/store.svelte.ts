// Shared reactive state for the receiver UI.
// All tabs, toolbar, and status bar read from this module.

import * as api from "./api";
import type {
  ConnectionsResponse,
  ForwarderReaderCountsUpdate,
  LastRead,
  ReceiverMode,
  StatusResponse,
  StreamCountUpdate,
  StreamsResponse,
} from "./api";
import {
  buildAllSubscriptions,
  buildUpdatedSubscriptions,
  type SubscriptionBuildStream,
} from "./subscriptions";
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
  | "connections"
  | "streams"
  | "config"
  | "announcer"
  | "logs"
  | "admin";

function subscriptionBuildStreams(
  streams: api.StreamEntry[],
): SubscriptionBuildStream[] {
  return streams.map((stream) => ({
    ...stream,
    local_port_override: stream.local_port_override ?? null,
  }));
}

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
  connections: null as ConnectionsResponse | null,
  error: null as string | null,

  // Streams. The per-stream maps below are keyed by canonical
  // `streamIdentity(...)` (forwarder_endpoint_id/stream_id), never by legacy
  // display metadata, which can collide across forwarders.
  streams: null as StreamsResponse | null,
  lastReads: new Map<string, LastRead>(),
  streamMetrics: new Map<string, api.StreamMetrics>(),
  streamActivityAt: new Map<string, number>(),

  // Forwarders
  forwarders: null as api.ForwarderEntry[] | null,
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
  // Where the effective server config comes from: "env" | "profile" | "none".
  // When "env", the server URL/token are overridden by environment variables
  // and the Config tab shows them read-only.
  serverSource: "none" as "env" | "profile" | "none",
  // Announcer publishing controls.
  announcerEnabled: false,
  announcerBusy: false,
  announcerMaxListSize: 25,
  announcerMaxListBusy: false,
  importBusy: false,
  importMessage: null as string | null,
  importError: null as string | null,
  // Paths of the most recently selected import files (display only). The Tauri
  // WebView may expose the absolute path; browser tests fall back to filename.
  participantsFilePath: null as string | null,
  chipsFilePath: null as string | null,
  // Counts describing the imported participant/chip data.
  dataStats: null as import("$lib/api").DataStats | null,
  saving: false,
  checkingUpdate: false,
  checkMessage: null as string | null,

  // Race Director output DBF config
  dbfEnabled: false,
  editDbfEnabled: false,
  dbfFlushIntervalMs: 1000,
  editDbfFlushIntervalMs: 1000,
  dbfSaving: false,
  dbfClearing: false,

  // Race Director participant/chip import config
  rdImportEnabled: false,
  rdImportDir: "C:\\Winrace\\Files",
  rdImportIntervalSecs: 15,
  editRdImportEnabled: false,
  editRdImportDir: "C:\\Winrace\\Files",
  editRdImportIntervalSecs: 15,
  rdImportSaving: false,

  // Update
  updateModalOpen: false,
  updateState: null as UpdateState | null,

  // Mode
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
  streamSubscriptionPendingSince: {} as Record<string, number>,
  streamEventTypeBusy: {} as Record<string, boolean>,
  streamAnnouncerBusy: {} as Record<string, boolean>,

  // UPS state (keyed by forwarder_id)
  upsState: new Map<
    string,
    { available: boolean; status: api.UpsStatus | null }
  >(),

  // Version info
  appVersion: "",
});

// Version tracking counters (stale-write guards) — not reactive, internal only
let modeHydrationVersion = 0;
let modeEditVersion = 0;
let modeMutationVersion = 0;
let streamRefreshVersion = 0;
let lastConcreteEpochByKey = new Map<string, number>();
const STREAM_ACTIVITY_RECENCY_MS = 10_000;
const STREAM_SUBSCRIBE_GRACE_MS = 10_000;

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

export type OverallHealth = "ok" | "warn" | "err";

export function getOverallHealth(): OverallHealth {
  const connections = store.connections;

  // With no connections payload, or no configured server yet, the roll-up is
  // unknown rather than healthy. Show warn until the receiver has enough data.
  if (!connections || !connections.server.configured) return "warn";

  const { server, forwarders } = connections;
  const intendedForwarders = forwarders.filter(
    (forwarder) => forwarder.pending || forwarder.state !== "disconnected",
  );
  const pendingForwarders = intendedForwarders.filter(
    (forwarder) => forwarder.pending,
  );
  const nonPendingIntendedForwarders = intendedForwarders.filter(
    (forwarder) => !forwarder.pending,
  );
  const unavailableForwarders = nonPendingIntendedForwarders.filter(
    (forwarder) => forwarder.state === "unavailable",
  );

  // Reader-level offline status is not available to this roll-up yet; Phase 3
  // should fold reader connectivity into the same err > warn > ok precedence.
  if (server.reachable === false) return "err";
  if (server.approval_state !== "active" && !server.waiting_for_approval) {
    return "err";
  }

  // Pending forwarders are still connecting, so they contribute warn/amber but
  // do not count as unavailable for the all-down/red health decision.
  if (
    nonPendingIntendedForwarders.length > 0 &&
    unavailableForwarders.length === nonPendingIntendedForwarders.length
  ) {
    return "err";
  }

  if (server.waiting_for_approval) return "warn";
  if (server.reachable !== true) return "warn";
  if (pendingForwarders.length > 0) return "warn";
  if (unavailableForwarders.length > 0) return "warn";

  return "ok";
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

export function openHelp(fieldKey: string): void {
  store.helpScrollTarget = fieldKey;
  store.showHelpModal = true;
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

export function streamKey(
  forwarder_id: string | null | undefined,
  reader_ip: string | null | undefined,
): string {
  return `${forwarder_id ?? ""}/${reader_ip ?? ""}`;
}

/// Canonical, always-present per-stream identity. Unlike `streamKey`, this
/// never collapses to `"/"` for canonical-only streams (those without legacy
/// `forwarder_id`/`reader_ip` metadata), so it is safe to use for UI row keys,
/// expand state, and per-stream input/saving maps.
export function streamIdentity(stream: {
  forwarder_endpoint_id: string;
  stream_id: string;
}): string {
  return `${stream.forwarder_endpoint_id}/${stream.stream_id}`;
}

export function streamHasRecentActivity(
  stream: Pick<api.StreamEntry, "forwarder_endpoint_id" | "stream_id">,
  now = Date.now(),
): boolean {
  const lastActivity = store.streamActivityAt.get(streamIdentity(stream));
  return (
    lastActivity != null && now - lastActivity <= STREAM_ACTIVITY_RECENCY_MS
  );
}

export function streamIsOptimisticallySubscribing(
  stream: Pick<
    api.StreamEntry,
    | "forwarder_endpoint_id"
    | "stream_id"
    | "subscribed"
    | "online"
    | "forwarder_id"
    | "reader_ip"
  >,
  now = Date.now(),
): boolean {
  if (!stream.subscribed || stream.online === true) return false;
  if (streamHasRecentActivity(stream, now)) return false;
  const pendingSince =
    store.streamSubscriptionPendingSince[streamIdentity(stream)];
  return (
    pendingSince != null && now - pendingSince <= STREAM_SUBSCRIBE_GRACE_MS
  );
}

function markSubscriptionPending(streams: api.StreamEntry[]): void {
  const now = Date.now();
  store.streamSubscriptionPendingSince = {
    ...store.streamSubscriptionPendingSince,
    ...Object.fromEntries(
      streams.map((stream) => [streamIdentity(stream), now]),
    ),
  };
}

function clearSubscriptionPending(streams: api.StreamEntry[]): void {
  const next = { ...store.streamSubscriptionPendingSince };
  let changed = false;
  const now = Date.now();
  for (const stream of streams) {
    const key = streamIdentity(stream);
    const settled =
      !stream.subscribed ||
      stream.online === true ||
      streamHasRecentActivity(stream, now);
    const expired =
      next[key] != null && now - next[key] > STREAM_SUBSCRIBE_GRACE_MS;
    if ((settled || expired) && key in next) {
      delete next[key];
      changed = true;
    }
  }
  if (changed) store.streamSubscriptionPendingSince = next;
}

function markStreamActivity(
  stream: Pick<api.StreamEntry, "forwarder_endpoint_id" | "stream_id">,
  now = Date.now(),
): void {
  const next = new Map(store.streamActivityAt);
  next.set(streamIdentity(stream), now);
  store.streamActivityAt = next;
}

/// Build a lookup from canonical stream identity to the live `StreamEntry`,
/// used to translate canonical-keyed input maps back to display metadata refs.
function streamsByIdentity(): Map<string, api.StreamEntry> {
  return new Map(
    (store.streams?.streams ?? []).map((s) => [streamIdentity(s), s]),
  );
}

function captureConcreteEpochs(
  existing: ReadonlyMap<string, number>,
  streams: readonly api.StreamEntry[],
): Map<string, number> {
  const result = new Map(existing);
  for (const stream of streams) {
    if (stream.stream_epoch != null) {
      result.set(streamIdentity(stream), stream.stream_epoch);
    }
  }
  return result;
}

function nextConcreteEpochs(
  previousConcreteEpochs: ReadonlyMap<string, number>,
  streams: readonly api.StreamEntry[],
): Map<string, number> {
  const next = new Map<string, number>();
  for (const stream of streams) {
    const key = streamIdentity(stream);
    if (stream.stream_epoch != null) {
      next.set(key, stream.stream_epoch);
    } else {
      const prev = previousConcreteEpochs.get(key);
      if (prev != null) {
        next.set(key, prev);
      }
    }
  }
  return next;
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
  const name = option.name?.trim() || "unnamed";
  const timestamp =
    option.created_unix_ms != null
      ? new Date(option.created_unix_ms).toLocaleString()
      : option.first_seen_at
        ? `first read ${new Date(option.first_seen_at).toLocaleString()}`
        : "created date unknown";
  return `#${option.stream_epoch} — ${name} — ${timestamp}`;
}

export function selectedEarliestEpochValue(stream: api.StreamEntry): string {
  const key = streamIdentity(stream);
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
    stream.stream_epoch != null &&
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
  const key = streamIdentity(stream);
  const configured = parseApiReturnedEpoch(key, store.targetedEpochInputs[key]);
  const options = store.earliestEpochOptions[key] ?? [];

  if (configured !== null) return String(configured);
  if (options.length === 0) return "";
  if (
    stream.stream_epoch != null &&
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
  const key = streamIdentity(stream);
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
  // The input maps are keyed by canonical stream identity. The compatibility
  // `ReceiverMode` payload is keyed by (forwarder_id, reader_ip), so resolve
  // each input back to its stream and only emit streams that still carry real
  // legacy metadata (canonical-only streams are not representable here).
  const byIdentity = streamsByIdentity();
  if (store.modeDraft === "targeted_replay") {
    const targets = Object.entries(store.targetedEpochInputs)
      .map(([key, value]) => {
        const stream = byIdentity.get(key);
        const stream_epoch = parseApiReturnedEpoch(key, value);
        if (
          !stream ||
          stream.forwarder_id == null ||
          stream.reader_ip == null ||
          stream_epoch === null
        )
          return null;
        return {
          forwarder_id: stream.forwarder_id,
          reader_ip: stream.reader_ip,
          stream_epoch,
        };
      })
      .filter((t): t is api.ReplayTarget => t !== null);
    return { mode: "targeted_replay", targets };
  }
  // Compatibility live mode is keyed by (forwarder_id, reader_ip); only include
  // streams that expose real display metadata rather than fabricating refs from
  // canonical identifiers.
  const liveStreams: api.StreamRef[] = (store.streams?.streams ?? [])
    .filter(
      (s): s is api.StreamEntry & { forwarder_id: string; reader_ip: string } =>
        s.forwarder_id != null && s.reader_ip != null,
    )
    .map((s) => ({
      forwarder_id: s.forwarder_id,
      reader_ip: s.reader_ip,
    }));
  const earliest_epochs = Object.entries(store.earliestEpochInputs)
    .map(([key, value]) => {
      const stream = byIdentity.get(key);
      const earliest_epoch = parseNonNegativeInt(value);
      if (
        !stream ||
        stream.forwarder_id == null ||
        stream.reader_ip == null ||
        earliest_epoch === null
      )
        return null;
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

type LoadAllOptions = {
  forceHydrateMode?: boolean;
};

// --------------- Actions ---------------

export async function prefetchEarliestEpochOptions(
  streamList: api.StreamEntry[],
  forceRefreshKeys: Set<string> = new Set(),
): Promise<void> {
  const tasks = streamList.map(async (stream) => {
    const key = streamIdentity(stream);
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
      // Replay-target epochs are looked up by canonical stream_id, matching
      // how the P2P data plane persists received events. When the durable
      // store has no events yet, fall back to the advertised current epoch so
      // the controls do not show "No epochs available".
      const response = await api.getReplayTargetEpochs({
        forwarder_endpoint_id: stream.forwarder_endpoint_id,
        stream_id: stream.stream_id,
      });
      const currentEpochName = stream.current_epoch_name?.trim() || null;
      const byEpoch = new Map<number, api.ReplayTargetEpochOption>();
      for (const option of response.epochs) {
        byEpoch.set(option.stream_epoch, option);
      }
      const advertisedOptions = [...(stream.epoch_options ?? [])];
      if (
        stream.stream_epoch != null &&
        !advertisedOptions.some(
          (option) => option.stream_epoch === stream.stream_epoch,
        )
      ) {
        advertisedOptions.unshift({
          stream_epoch: stream.stream_epoch,
          created_unix_ms: stream.current_epoch_created_unix_ms ?? null,
        });
      }
      for (const option of advertisedOptions) {
        const existing = byEpoch.get(option.stream_epoch);
        const isCurrent = option.stream_epoch === stream.stream_epoch;
        byEpoch.set(option.stream_epoch, {
          stream_epoch: option.stream_epoch,
          name: existing?.name?.trim()
            ? existing.name
            : isCurrent
              ? currentEpochName
              : null,
          first_seen_at: existing?.first_seen_at ?? null,
          created_unix_ms: isCurrent
            ? (stream.current_epoch_created_unix_ms ??
              option.created_unix_ms ??
              existing?.created_unix_ms ??
              null)
            : (option.created_unix_ms ?? existing?.created_unix_ms ?? null),
          race_names: existing?.race_names ?? [],
        });
      }
      store.earliestEpochOptions = {
        ...store.earliestEpochOptions,
        [key]: [...byEpoch.values()].sort(
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
  // Saved modes are keyed by legacy (forwarder_id, reader_ip); translate to
  // canonical stream identity using the current stream list so the input maps
  // stay canonical-keyed. When the matching stream is not currently known we
  // fall back to the legacy streamKey rather than dropping the override.
  const identityForLegacyRef = (
    forwarder_id: string,
    reader_ip: string,
  ): string => {
    const stream = (store.streams?.streams ?? []).find(
      (s) => s.forwarder_id === forwarder_id && s.reader_ip === reader_ip,
    );
    return stream ? streamIdentity(stream) : streamKey(forwarder_id, reader_ip);
  };
  if (mode.mode === "live") {
    const rows = Array.isArray(mode.earliest_epochs)
      ? mode.earliest_epochs
      : [];
    store.earliestEpochInputs = Object.fromEntries(
      rows.map((r) => [
        identityForLegacyRef(r.forwarder_id, r.reader_ip),
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
      identityForLegacyRef(t.forwarder_id, t.reader_ip),
      String(t.stream_epoch),
    ]),
  );
}

function resetHydratedMode(): void {
  hydrateMode({ mode: "live", streams: [], earliest_epochs: [] });
  store.savedModePayload = JSON.stringify({
    mode: "live",
    streams: [],
    earliest_epochs: [],
  });
  store.modeEditedSinceHydration = false;
  modeHydrationVersion += 1;
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

  const knownKeys = new Set(store.streams.streams.map(streamIdentity));
  const updatesByKey = new Map(updates.map((u) => [streamIdentity(u), u]));
  const hasUnknown = updates.some((u) => !knownKeys.has(streamIdentity(u)));

  store.streams = {
    ...store.streams,
    streams: store.streams.streams.map((s) => {
      if (!s.subscribed) return s;
      const u = updatesByKey.get(streamIdentity(s));
      if (!u) return s;
      return { ...s, reads_total: u.reads_total, reads_epoch: u.reads_epoch };
    }),
  };
  return hasUnknown;
}

export async function loadConnections(): Promise<void> {
  try {
    store.connections = await api.getConnections();
  } catch (e) {
    store.error = String(e);
  }
}

export async function refreshStreamsAndEpochOptions(
  refreshStreams: api.StreamIdentity[] = [],
): Promise<void> {
  const refreshVersion = ++streamRefreshVersion;
  try {
    const latestStreams = await api.getStreams();
    if (refreshVersion === streamRefreshVersion) {
      store.streams = latestStreams;
      clearSubscriptionPending(latestStreams.streams);
      const refreshEpochOptionKeys = new Set(
        refreshStreams.map(streamIdentity),
      );
      await prefetchEarliestEpochOptions(
        latestStreams.streams,
        refreshEpochOptionKeys,
      );
    }
  } catch (e) {
    store.error = String(e);
  }
}

export async function loadAll(options: LoadAllOptions = {}): Promise<void> {
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
      nextConnections,
      nextStreams,
      nextLogs,
      nextMode,
      nextMetrics,
      nextDataStats,
    ] = await Promise.all([
      api.getStatus(),
      api.getConnections().catch(() => null),
      api.getStreams(),
      api.getLogs(),
      api.getMode().catch(() => null),
      api.getStreamMetrics().catch((e: unknown) => {
        console.warn(
          "getStreamMetrics failed, will rely on real-time updates:",
          e,
        );
        return [] as api.StreamMetrics[];
      }),
      api.getDataStats().catch((e: unknown) => {
        console.warn("getDataStats failed, leaving cached data stats:", e);
        return null;
      }),
    ]);

    await Promise.all([loadDbfConfig(), loadRdImportConfig()]);

    store.status = nextStatus;
    if (nextConnections) {
      store.connections = nextConnections;
    }
    if (streamRefreshVersion === streamRefreshVersionAtStart) {
      store.streams = nextStreams;
      clearSubscriptionPending(nextStreams.streams);
      lastConcreteEpochByKey = nextConcreteEpochs(
        lastConcreteEpochByKey,
        nextStreams.streams,
      );
      void prefetchEarliestEpochOptions(nextStreams.streams);
    }
    if (nextMetrics.length > 0) {
      const merged = new Map(store.streamMetrics);
      for (const m of nextMetrics) {
        merged.set(streamIdentity(m), m);
      }
      store.streamMetrics = merged;
    }
    if (nextDataStats) {
      store.dataStats = nextDataStats;
    }
    store.logEntries = nextLogs.entries;
    store.forwarders = null;
    store.forwardersError = null;
    store.selectedForwarderId = null;
    if (
      options.forceHydrateMode ||
      (!getModeDirty() &&
        modeEditVersion === modeEditVersionAtStart &&
        modeHydrationVersion === modeVersionAtStart &&
        modeMutationVersion === modeMutationVersionAtStart)
    ) {
      if (nextMode) {
        applyHydratedMode(nextMode);
      } else {
        resetHydratedMode();
      }
    }

    const p = await api.getProfile().catch(() => null);
    if (p) {
      const configWasDirty = getConfigDirty();
      store.savedServerUrl = p.server_url;
      store.savedToken = p.token;
      store.savedReceiverId = p.receiver_id;
      store.serverSource = p.server_source ?? "none";
      store.announcerEnabled = p.announcer_enabled ?? false;
      store.announcerMaxListSize = p.announcer_max_list_size ?? 25;
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
  const key = streamIdentity(stream);
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
      forwarder_endpoint_id: stream.forwarder_endpoint_id,
      stream_id: stream.stream_id,
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
      allStreams: subscriptionBuildStreams(store.streams.streams),
      target: {
        forwarder_endpoint_id: stream.forwarder_endpoint_id,
        stream_id: stream.stream_id,
        currentlySubscribed: stream.subscribed,
      },
    });
    if (result.error) {
      store.error = result.error;
      return;
    }
    if (!stream.subscribed) {
      markSubscriptionPending([stream]);
    } else {
      clearSubscriptionPending([{ ...stream, subscribed: false }]);
    }
    await api.putSubscriptions(result.subscriptions!);
    const latestStreams = await api.getStreams();
    if (refreshVersion === streamRefreshVersion) {
      store.streams = latestStreams;
      clearSubscriptionPending(latestStreams.streams);
      void prefetchEarliestEpochOptions(latestStreams.streams);
    }
  } catch (e) {
    if (!stream.subscribed) {
      clearSubscriptionPending([{ ...stream, subscribed: false }]);
    }
    store.error = String(e);
  } finally {
    store.streamActionBusy = false;
  }
}

export async function subscribeAllAvailable(): Promise<void> {
  if (store.streamActionBusy || !store.streams) return;
  if (!store.streams.streams.some((stream) => !stream.subscribed)) return;

  const pendingStreams = store.streams.streams.filter(
    (stream) => !stream.subscribed,
  );
  store.streamActionBusy = true;
  const refreshVersion = ++streamRefreshVersion;
  try {
    store.error = null;
    markSubscriptionPending(pendingStreams);
    await api.putSubscriptions(
      buildAllSubscriptions(subscriptionBuildStreams(store.streams.streams)),
    );
    const latestStreams = await api.getStreams();
    if (refreshVersion === streamRefreshVersion) {
      store.streams = latestStreams;
      clearSubscriptionPending(latestStreams.streams);
      void prefetchEarliestEpochOptions(latestStreams.streams);
    }
  } catch (e) {
    clearSubscriptionPending(pendingStreams);
    store.error = String(e);
  } finally {
    store.streamActionBusy = false;
  }
}

export async function updateStreamEventType(
  stream: api.StreamEntry,
  eventType: "start" | "finish",
): Promise<void> {
  if (!store.streams || !stream.subscribed) return;

  const key = streamIdentity(stream);
  if (store.streamEventTypeBusy[key]) return;

  store.streamEventTypeBusy = { ...store.streamEventTypeBusy, [key]: true };
  try {
    store.error = null;
    await api.updateSubscriptionEventType(
      {
        forwarder_endpoint_id: stream.forwarder_endpoint_id,
        stream_id: stream.stream_id,
      },
      eventType,
    );
    store.streams = {
      ...store.streams,
      streams: store.streams.streams.map((candidate) =>
        candidate.forwarder_endpoint_id === stream.forwarder_endpoint_id &&
        candidate.stream_id === stream.stream_id
          ? { ...candidate, event_type: eventType }
          : candidate,
      ),
    };
  } catch (e) {
    store.error = String(e);
  } finally {
    store.streamEventTypeBusy = { ...store.streamEventTypeBusy, [key]: false };
  }
}

export async function replayStream(stream: api.StreamEntry): Promise<void> {
  const parsed = resolveReplayTargetEpoch(stream);
  if (parsed === null) {
    store.error = "Select a valid target epoch before replaying.";
    return;
  }
  // Targeted replay still uses display metadata keyed by (forwarder_id, reader_ip).
  const { forwarder_id, reader_ip } = stream;
  if (forwarder_id == null || reader_ip == null) {
    store.error = "Stream is missing legacy metadata required for replay.";
    return;
  }
  try {
    store.error = null;
    const payload: ReceiverMode = {
      mode: "targeted_replay",
      targets: [
        {
          forwarder_id,
          reader_ip,
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
      const { forwarder_id, reader_ip } = s;
      if (forwarder_id == null || reader_ip == null) return null;
      return {
        forwarder_id,
        reader_ip,
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

export async function reconnectServer(): Promise<void> {
  try {
    store.error = null;
    await api.reconnectServer();
    await loadAll();
  } catch (e) {
    store.error = `Failed to reconnect server: ${e}`;
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

export async function setAnnouncerEnabled(enabled: boolean): Promise<void> {
  store.announcerBusy = true;
  try {
    store.error = null;
    await api.setAnnouncerEnabled(enabled);
    store.announcerEnabled = enabled;
  } catch (e) {
    store.error = `Failed to update announcer setting: ${e}`;
  } finally {
    store.announcerBusy = false;
  }
}

export async function setAnnouncerMaxListSize(
  maxListSize: number,
): Promise<void> {
  // Clamp to the same range the backend enforces so the UI does not send
  // obviously-invalid values.
  const clamped = Math.min(500, Math.max(1, Math.round(maxListSize)));
  store.announcerMaxListBusy = true;
  try {
    store.error = null;
    await api.setAnnouncerMaxListSize(clamped);
    store.announcerMaxListSize = clamped;
  } catch (e) {
    store.error = `Failed to update announcer list size: ${e}`;
  } finally {
    store.announcerMaxListBusy = false;
  }
}

export async function setStreamAnnouncerPublish(
  stream: api.StreamEntry,
  publish: boolean,
): Promise<void> {
  const key = streamIdentity(stream);
  // Guard against rapid re-toggles: a second click while a request is in
  // flight would race the refetch and could clobber the latest state.
  if (store.streamAnnouncerBusy[key]) return;
  store.streamAnnouncerBusy = { ...store.streamAnnouncerBusy, [key]: true };
  try {
    store.error = null;
    await api.setStreamAnnouncerPublish(
      stream.forwarder_endpoint_id,
      stream.stream_id,
      publish,
    );
    // Refresh streams so the toggle reflects persisted state.
    const latest = await api.getStreams();
    store.streams = latest;
  } catch (e) {
    store.error = `Failed to update stream announcer setting: ${e}`;
  } finally {
    store.streamAnnouncerBusy = { ...store.streamAnnouncerBusy, [key]: false };
  }
}

export async function loadDataStats(): Promise<void> {
  try {
    store.dataStats = await api.getDataStats();
  } catch (e) {
    console.error("Failed to load data stats:", e);
  }
}

export async function importParticipantsFile(filePath: string): Promise<void> {
  store.importBusy = true;
  store.importMessage = null;
  store.importError = null;
  try {
    const summary = await api.importParticipantsFile(filePath);
    store.participantsFilePath = filePath;
    store.importMessage = `Imported ${summary.imported} participant(s).`;
    await loadDataStats();
  } catch (e) {
    store.importError = `Participant import failed: ${e}`;
  } finally {
    store.importBusy = false;
  }
}

export async function importChipsFile(filePath: string): Promise<void> {
  store.importBusy = true;
  store.importMessage = null;
  store.importError = null;
  try {
    const summary = await api.importChipsFile(filePath);
    store.chipsFilePath = filePath;
    store.importMessage = `Imported ${summary.imported} chip assignment(s).`;
    await loadDataStats();
  } catch (e) {
    store.importError = `Chip import failed: ${e}`;
  } finally {
    store.importBusy = false;
  }
}

export async function loadDbfConfig() {
  try {
    const config = await api.getDbfConfig();
    store.dbfEnabled = config.enabled;
    store.editDbfEnabled = config.enabled;
    store.dbfFlushIntervalMs = config.flush_interval_ms ?? 1000;
    store.editDbfFlushIntervalMs = store.dbfFlushIntervalMs;
  } catch (e) {
    console.error("Failed to load DBF config:", e);
    store.error = `Failed to load DBF config: ${e}`;
  }
}

export async function saveDbfConfig() {
  store.dbfSaving = true;
  try {
    await api.putDbfConfig({
      enabled: store.editDbfEnabled,
      flush_interval_ms: store.editDbfFlushIntervalMs,
    });
    store.dbfEnabled = store.editDbfEnabled;
    store.dbfFlushIntervalMs = store.editDbfFlushIntervalMs;
  } catch (e) {
    store.error = `Failed to save DBF config: ${e}`;
  } finally {
    store.dbfSaving = false;
  }
}

export async function loadRdImportConfig() {
  try {
    const config = await api.getRdImportConfig();
    store.rdImportEnabled = config.enabled;
    store.rdImportDir = config.dir;
    store.rdImportIntervalSecs = config.interval_secs;
    store.editRdImportEnabled = config.enabled;
    store.editRdImportDir = config.dir;
    store.editRdImportIntervalSecs = config.interval_secs;
  } catch (e) {
    console.error("Failed to load Race Director import config:", e);
    store.error = `Failed to load Race Director import config: ${e}`;
  }
}

export async function saveRdImportConfig() {
  store.rdImportSaving = true;
  try {
    await api.putRdImportConfig({
      enabled: store.editRdImportEnabled,
      dir: store.editRdImportDir,
      interval_secs: store.editRdImportIntervalSecs,
    });
    store.rdImportEnabled = store.editRdImportEnabled;
    store.rdImportDir = store.editRdImportDir;
    store.rdImportIntervalSecs = store.editRdImportIntervalSecs;
  } catch (e) {
    store.error = `Failed to save Race Director import config: ${e}`;
  } finally {
    store.rdImportSaving = false;
  }
}

export async function clearDbfFile() {
  store.dbfClearing = true;
  try {
    await api.clearDbf();
  } catch (e) {
    store.error = `Failed to clear DBF file: ${e}`;
  } finally {
    store.dbfClearing = false;
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

/// Patch a reader's volatile counters (session/total reads, last seen) into
/// the cached connections payload in place. Count refreshes arrive as targeted
/// events so they don't trigger a full connections reload; unknown
/// forwarders/readers are ignored because structural changes still fire
/// ConnectionsChanged.
function applyForwarderReaderCountsUpdate(
  update: ForwarderReaderCountsUpdate,
): void {
  const connections = store.connections;
  if (!connections) return;
  let changed = false;
  const forwarders = connections.forwarders.map((forwarder) => {
    if (forwarder.endpoint_id !== update.forwarder_id) return forwarder;
    const readers = forwarder.readers.map((reader) => {
      if (reader.stream_id !== update.stream_id) return reader;
      changed = true;
      return {
        ...reader,
        reads_session: update.reads_session,
        reads_epoch: update.reads_epoch ?? reader.reads_epoch,
        reads_total: update.reads_total,
        last_read_unix_ms: update.last_read_unix_ms,
        last_seen_secs: update.last_seen_secs,
      };
    });
    return { ...forwarder, readers };
  });
  if (changed) {
    store.connections = { ...connections, forwarders };
  }
}

function applyForwarderUpsUpdate(
  forwarderId: string,
  available: boolean,
  status: api.UpsStatus | null,
): void {
  const next = new Map(store.upsState);
  next.set(forwarderId, { available, status });
  store.upsState = next;
}

function pruneUpsStateForOnlineForwarders(
  forwarders: api.ForwarderEntry[] | null,
): void {
  if (!forwarders) {
    store.upsState = new Map();
    return;
  }

  const onlineForwarders = new Set(
    forwarders
      .filter((forwarder) => forwarder.online)
      .map((forwarder) => forwarder.forwarder_id),
  );

  store.upsState = new Map(
    Array.from(store.upsState.entries()).filter(([forwarderId]) =>
      onlineForwarders.has(forwarderId),
    ),
  );
}

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
      const serverStatus = store.status?.server ?? {
        configured: false,
        endpoint_id: null,
        reachable: null,
        approval_state: null,
        waiting_for_approval: false,
        message: null,
      };
      store.status = { ...s, server: serverStatus };
      if (s.connection_state === "disconnected") {
        store.streamMetrics = new Map();
      }
    },
    onStreamsSnapshot: (s) => {
      const previousStreams = store.streams?.streams ?? [];
      const previousEpochByKey = new Map(
        previousStreams.map((st) => [streamIdentity(st), st.stream_epoch]),
      );
      const previousConcreteEpochByKey = captureConcreteEpochs(
        lastConcreteEpochByKey,
        previousStreams,
      );
      const previousIdentities = new Set(previousStreams.map(streamIdentity));
      const refreshEpochOptionKeys = new Set<string>();
      for (const stream of s.streams) {
        const identity = streamIdentity(stream);
        if (!previousIdentities.has(identity)) {
          refreshEpochOptionKeys.add(identity);
          continue;
        }
        if (stream.stream_epoch == null) continue;
        const lastKnown =
          previousEpochByKey.get(identity) ??
          previousConcreteEpochByKey.get(identity);
        if (lastKnown != null && lastKnown !== stream.stream_epoch) {
          refreshEpochOptionKeys.add(identity);
        }
      }
      streamRefreshVersion += 1;
      store.streams = s;
      clearSubscriptionPending(s.streams);
      void prefetchEarliestEpochOptions(s.streams, refreshEpochOptionKeys);
      // Prune stale metrics
      const currentKeys = new Set(s.streams.map(streamIdentity));
      const prunedMetrics = new Map(store.streamMetrics);
      for (const key of prunedMetrics.keys()) {
        if (!currentKeys.has(key)) prunedMetrics.delete(key);
      }
      for (const stream of s.streams) {
        if (stream.stream_epoch == null) continue;
        const identity = streamIdentity(stream);
        const lastKnown =
          previousEpochByKey.get(identity) ??
          previousConcreteEpochByKey.get(identity);
        if (lastKnown != null && lastKnown !== stream.stream_epoch) {
          prunedMetrics.delete(identity);
        }
      }
      lastConcreteEpochByKey = nextConcreteEpochs(
        previousConcreteEpochByKey,
        s.streams,
      );
      store.streamMetrics = prunedMetrics;
    },
    onLogEntry: (entry) => {
      store.logEntries = [entry, ...store.logEntries].slice(0, 500);
    },
    onResync: () => {
      void loadAll();
    },
    onConnectionsChanged: () => {
      void loadConnections();
    },
    onConnectionChange: () => {},
    onForwarderReaderCountsUpdated: (update) => {
      applyForwarderReaderCountsUpdate(update);
    },
    onModeChanged: (mode) => {
      applyHydratedMode(mode);
    },
    onStreamDeltas: (updates) => {
      if (updates.length === 0) return;
      // Counts (keyed row update; unknown streams trigger a resync/reload,
      // same as the legacy per-stream counts event).
      const needsResync = applyStreamCountUpdates(
        updates.map((u) => ({
          forwarder_endpoint_id: u.forwarder_endpoint_id,
          stream_id: u.stream_id,
          reads_total: u.reads_total,
          reads_epoch: u.reads_epoch,
        })),
      );
      // Activity is keyed by canonical stream identity so streams that share
      // legacy display metadata do not borrow each other's optimistic windows.
      for (const update of updates) {
        markStreamActivity(update);
      }
      if (store.streams) clearSubscriptionPending(store.streams.streams);
      if (needsResync) void loadAll();
      // Metrics + last read, keyed by canonical stream identity so streams
      // that share legacy display metadata never clobber each other.
      const nextMetrics = new Map(store.streamMetrics);
      const nextReads = new Map(store.lastReads);
      for (const u of updates) {
        const key = streamIdentity(u);
        nextMetrics.set(key, u.metrics);
        if (u.last_read) {
          nextReads.set(key, {
            forwarder_id: u.last_read.forwarder_id,
            reader_ip: u.last_read.reader_ip,
            chip_id: u.last_read.chip_id,
            timestamp: u.last_read.timestamp,
            bib: u.last_read.bib ?? null,
            name: u.last_read.name ?? null,
          });
        }
      }
      store.streamMetrics = nextMetrics;
      store.lastReads = nextReads;
    },
    onForwarderUpsUpdated: (payload) => {
      applyForwarderUpsUpdate(
        payload.forwarder_id,
        payload.available,
        payload.status,
      );
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
