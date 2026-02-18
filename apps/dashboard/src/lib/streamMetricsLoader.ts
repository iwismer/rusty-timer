import type { StreamMetrics } from "./api";

export function shouldFetchMetrics(
  streamId: string,
  metricsByStream: Record<string, StreamMetrics>,
  requestedStreamIds: Set<string>,
  inFlightStreamIds: Set<string>,
): boolean {
  if (!streamId) return false;
  if (metricsByStream[streamId]) return false;
  if (requestedStreamIds.has(streamId)) return false;
  if (inFlightStreamIds.has(streamId)) return false;
  return true;
}
