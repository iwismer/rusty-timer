import { describe, expect, it } from "vitest";
import type { StreamMetrics } from "./api";
import { shouldFetchMetrics } from "./streamMetricsLoader";

const SAMPLE_METRICS: StreamMetrics = {
  raw_count: 10,
  dedup_count: 8,
  retransmit_count: 2,
  lag: 100,
  backlog: 0,
};

describe("shouldFetchMetrics", () => {
  it("returns true when stream has no metrics and no request state", () => {
    expect(shouldFetchMetrics("s1", {}, new Set(), new Set())).toBe(true);
  });

  it("returns false for empty stream id", () => {
    expect(shouldFetchMetrics("", {}, new Set(), new Set())).toBe(false);
  });

  it("returns false when metrics already exist", () => {
    expect(
      shouldFetchMetrics("s1", { s1: SAMPLE_METRICS }, new Set(), new Set()),
    ).toBe(false);
  });

  it("returns false when request for stream is already in flight", () => {
    expect(shouldFetchMetrics("s1", {}, new Set(), new Set(["s1"]))).toBe(
      false,
    );
  });

  it("returns false when stream was already requested", () => {
    expect(shouldFetchMetrics("s1", {}, new Set(["s1"]), new Set())).toBe(
      false,
    );
  });

  it("returns true when navigating to a different stream id", () => {
    expect(shouldFetchMetrics("s2", {}, new Set(["s1"]), new Set())).toBe(
      true,
    );
  });
});
