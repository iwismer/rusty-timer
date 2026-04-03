import {
  addOrUpdateStream,
  forwarderRacesStore,
  patchStream,
  pruneUpsStateForOnlineForwarders,
  replaceStreams,
  setRaces,
  setMetrics,
  setForwarderRace,
  pushLog,
  logsStore,
  readerStatesStore,
  streamsStore,
  setReaderState,
  setDownloadProgress,
  setUpsState,
} from "./stores";
import { get } from "svelte/store";
import {
  getForwarderRaces,
  getLogs,
  getRaces,
  getReaderStates,
  getStreams,
} from "./api";
import type { CachedReaderState, StreamEntry, StreamMetrics } from "./api";
import { mergeLogsWithPendingLive } from "./logs-merge";

type StreamUpdatedEvent = {
  stream_id: string;
  stream_epoch?: number;
  online?: boolean;
  reader_connected?: boolean;
  display_alias?: string | null;
  forwarder_display_name?: string | null;
};

type StreamUpdatedListener = (update: StreamUpdatedEvent) => void;

let eventSource: EventSource | null = null;
let resyncInFlight = false;
let resyncQueued = false;
let logsResyncInFlight = false;
let pendingLiveLogs: string[] = [];
const streamUpdatedListeners = new Set<StreamUpdatedListener>();

function replaceForwarderAssignments(
  assignments: Array<{ forwarder_id: string; race_id: string | null }>,
): void {
  const next: Record<string, string | null> = {};
  for (const assignment of assignments) {
    next[assignment.forwarder_id] = assignment.race_id;
  }
  forwarderRacesStore.set(next);
}

export function initSSE(): void {
  if (eventSource) return;

  eventSource = new EventSource("/api/v1/events");

  eventSource.addEventListener("stream_created", (e: MessageEvent) => {
    try {
      const stream: StreamEntry = JSON.parse(e.data);
      addOrUpdateStream(stream);
    } catch (err) {
      console.error("failed to parse stream_created event:", err);
    }
  });

  eventSource.addEventListener("stream_updated", (e: MessageEvent) => {
    try {
      const update: StreamUpdatedEvent = JSON.parse(e.data);
      const { stream_id, ...fields } = update;
      patchStream(stream_id, fields);
      pruneUpsStateForOnlineForwarders(get(streamsStore));
      for (const listener of streamUpdatedListeners) {
        listener(update);
      }
    } catch (err) {
      console.error("failed to parse stream_updated event:", err);
    }
  });

  eventSource.addEventListener("metrics_updated", (e: MessageEvent) => {
    try {
      const data = JSON.parse(e.data);
      const metrics: StreamMetrics = {
        raw_count: data.raw_count,
        dedup_count: data.dedup_count,
        retransmit_count: data.retransmit_count,
        lag: data.lag_ms ?? null,
        backlog: 0,
        epoch_raw_count: data.epoch_raw_count,
        epoch_dedup_count: data.epoch_dedup_count,
        epoch_retransmit_count: data.epoch_retransmit_count,
        epoch_lag: data.epoch_lag_ms ?? null,
        epoch_last_received_at: data.epoch_last_received_at ?? null,
        unique_chips: data.unique_chips,
        last_tag_id: data.last_tag_id ?? null,
        last_reader_timestamp: data.last_reader_timestamp ?? null,
      };
      setMetrics(data.stream_id, metrics);
    } catch (err) {
      console.error("failed to parse metrics_updated event:", err);
    }
  });

  eventSource.addEventListener("forwarder_race_assigned", (e: MessageEvent) => {
    try {
      const data = JSON.parse(e.data);
      setForwarderRace(data.forwarder_id, data.race_id ?? null);
    } catch (err) {
      console.error("failed to parse forwarder_race_assigned event:", err);
    }
  });

  eventSource.addEventListener("log_entry", (e: MessageEvent) => {
    try {
      const data = JSON.parse(e.data);
      pushLog(data.entry);
      if (logsResyncInFlight) {
        const entry = String(data.entry ?? "").trim();
        if (entry) pendingLiveLogs.push(entry);
      }
    } catch (err) {
      console.error("failed to parse log_entry event:", err);
    }
  });

  eventSource.addEventListener("reader_info_updated", (e: MessageEvent) => {
    try {
      const data = JSON.parse(e.data);
      const key = `${data.forwarder_id}:${data.reader_ip}`;
      setReaderState(key, {
        forwarder_id: data.forwarder_id,
        reader_ip: data.reader_ip,
        state: data.state,
        reader_info: data.reader_info,
      });
    } catch (err) {
      console.error("Failed to parse reader_info_updated event:", err);
    }
  });

  eventSource.addEventListener(
    "reader_download_progress",
    (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data);
        const key = `${data.forwarder_id}:${data.reader_ip}`;
        setDownloadProgress(key, data);
      } catch (err) {
        console.error("Failed to parse reader_download_progress event:", err);
      }
    },
  );

  eventSource.addEventListener("forwarder_ups_updated", (e: MessageEvent) => {
    try {
      const data = JSON.parse(e.data);
      setUpsState(data.forwarder_id, data.available, data.status ?? null);
    } catch (err) {
      console.error("Failed to parse forwarder_ups_updated event:", err);
    }
  });

  eventSource.addEventListener("resync", async () => {
    await resync();
  });

  eventSource.onopen = async () => {
    await resync();
  };

  // Eagerly fetch dashboard state without waiting for the SSE connection to open.
  // When no forwarders are connected, the SSE response body has no data
  // until the first keep-alive (15 s), which can delay the onopen callback.
  void resync();
}

export function onStreamUpdated(listener: StreamUpdatedListener): () => void {
  streamUpdatedListeners.add(listener);
  return () => {
    streamUpdatedListeners.delete(listener);
  };
}

async function resync(): Promise<void> {
  if (resyncInFlight) {
    resyncQueued = true;
    return;
  }

  resyncInFlight = true;
  logsResyncInFlight = true;
  try {
    // Coalesce multiple resync triggers into a single follow-up fetch.
    while (true) {
      resyncQueued = false;
      const [
        streamsResp,
        racesResp,
        assignmentsResp,
        logsResp,
        readerStatesResp,
      ] = await Promise.allSettled([
        getStreams(),
        getRaces(),
        getForwarderRaces(),
        getLogs(),
        getReaderStates(),
      ]);

      if (streamsResp.status === "rejected") {
        console.error("resync: failed to fetch streams", streamsResp.reason);
      }
      if (racesResp.status === "rejected") {
        console.error("resync: failed to fetch races", racesResp.reason);
      }
      if (assignmentsResp.status === "rejected") {
        console.error(
          "resync: failed to fetch forwarder races",
          assignmentsResp.reason,
        );
      }
      if (logsResp.status === "rejected") {
        console.error("resync: failed to fetch logs", logsResp.reason);
      }
      if (readerStatesResp.status === "rejected") {
        console.error(
          "resync: failed to fetch reader states",
          readerStatesResp.reason,
        );
      }

      if (streamsResp.status === "fulfilled") {
        replaceStreams(streamsResp.value.streams);
        pruneUpsStateForOnlineForwarders(streamsResp.value.streams);
      }
      if (racesResp.status === "fulfilled") {
        setRaces(racesResp.value.races);
      }
      if (assignmentsResp.status === "fulfilled") {
        replaceForwarderAssignments(assignmentsResp.value.assignments);
      }
      if (logsResp.status === "fulfilled") {
        logsStore.set(
          mergeLogsWithPendingLive(
            logsResp.value.entries,
            pendingLiveLogs,
            500,
          ),
        );
        pendingLiveLogs = [];
      }
      if (readerStatesResp.status === "fulfilled") {
        const next: Record<string, CachedReaderState> = {};
        for (const rs of readerStatesResp.value) {
          next[`${rs.forwarder_id}:${rs.reader_ip}`] = rs;
        }
        readerStatesStore.set(next);
      }
      if (!resyncQueued) break;
    }
  } finally {
    resyncInFlight = false;
    logsResyncInFlight = false;
  }
}

export function destroySSE(): void {
  if (eventSource) {
    eventSource.close();
    eventSource = null;
  }
  resyncInFlight = false;
  resyncQueued = false;
  logsResyncInFlight = false;
  pendingLiveLogs = [];
}
