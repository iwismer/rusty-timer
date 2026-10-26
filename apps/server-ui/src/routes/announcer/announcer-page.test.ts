import { describe, expect, it } from "vitest";
import announcerSource from "./+page.svelte?raw";
import type { AnnouncerRow } from "$lib/api";
import {
  DEFAULT_EXPIRY_MINUTES,
  EXPIRY_STORAGE_KEY,
  clampExpiryMinutes,
  filterVisibleAnnouncerRows,
  isRowWithinExpiry,
  loadExpiryMinutes,
  saveExpiryMinutes,
} from "./announcer-helpers";

function makeRow(
  seq: number,
  receivedAt: string,
  overrides: Partial<AnnouncerRow> = {},
): AnnouncerRow {
  return {
    forwarder_endpoint_id: "fwd-1",
    stream_id: "finish-line",
    seq,
    chip_id: `chip-${seq}`,
    bib: 100 + seq,
    display_name: `Runner ${seq}`,
    reader_timestamp: null,
    received_at: receivedAt,
    division: "5k",
    gender: "F",
    age: 29,
    ...overrides,
  };
}

describe("server announcer page reset controls", () => {
  it("includes reset button and confirmation flow in template", () => {
    expect(announcerSource).toContain('data-testid="announcer-reset-btn"');
    expect(announcerSource).toContain(
      'data-testid="confirm-announcer-reset-btn"',
    );
    expect(announcerSource).toContain("api.resetAnnouncer()");
  });

  it("resets local keys and status when reset succeeds", () => {
    expect(announcerSource).toContain("seenKeys.clear()");
    expect(announcerSource).toContain("seeded = false");
    expect(announcerSource).toContain("flashKeys = new Set()");
    expect(announcerSource).toContain("finisher_count: 0");
    expect(announcerSource).toContain("announcer_rows: []");
  });

  it("handles remote generation changes in loadStatus", () => {
    expect(announcerSource).toContain(
      "next.announcer_source_generation !== lastGeneration",
    );
  });
});

describe("server announcer page participant fields and expiry", () => {
  it("renders race/division, gender, bib, and age in announcer rows", () => {
    expect(announcerSource).toContain('data-testid="announcer-row-division"');
    expect(announcerSource).toContain("Race/Division {row.division}");
    expect(announcerSource).toContain('data-testid="announcer-row-gender"');
    expect(announcerSource).toContain("Gender {row.gender}");
    expect(announcerSource).toContain('data-testid="announcer-row-bib"');
    expect(announcerSource).toContain("Bib {row.bib}");
    expect(announcerSource).toContain('data-testid="announcer-row-age"');
    expect(announcerSource).toContain("Age {row.age}");
  });

  it("includes configurable expiry input defaulting to 15 minutes", () => {
    expect(DEFAULT_EXPIRY_MINUTES).toBe(15);
    expect(announcerSource).toContain(
      'data-testid="announcer-expiry-minutes-input"',
    );
    expect(announcerSource).toContain("filterVisibleAnnouncerRows");
  });

  it("hides athletes older than the configured expiry window and sorts newest first", () => {
    const nowMs = Date.parse("2026-07-02T12:20:00Z");
    const freshRow = makeRow(3, "2026-07-02T12:18:00Z"); // 2 mins old
    const boundaryRow = makeRow(2, "2026-07-02T12:05:00Z"); // 15 mins old
    const expiredRow = makeRow(1, "2026-07-02T12:04:59Z"); // 15m01s old

    const visibleDefault = filterVisibleAnnouncerRows(
      [expiredRow, boundaryRow, freshRow],
      nowMs,
      DEFAULT_EXPIRY_MINUTES,
    );
    expect(visibleDefault.map((r) => r.seq)).toEqual([3, 2]);

    const visibleShort = filterVisibleAnnouncerRows(
      [expiredRow, boundaryRow, freshRow],
      nowMs,
      5,
    );
    expect(visibleShort.map((r) => r.seq)).toEqual([3]);

    const visibleLong = filterVisibleAnnouncerRows(
      [expiredRow, boundaryRow, freshRow],
      nowMs,
      30,
    );
    expect(visibleLong.map((r) => r.seq)).toEqual([3, 2, 1]);
  });

  it("persists and clamps configured expiry minutes in storage", () => {
    const map = new Map<string, string>();
    const fakeStorage = {
      getItem: (key: string) => map.get(key) ?? null,
      setItem: (key: string, val: string) => {
        map.set(key, val);
      },
    };

    expect(loadExpiryMinutes(fakeStorage)).toBe(15);
    expect(saveExpiryMinutes(25, fakeStorage)).toBe(25);
    expect(map.get(EXPIRY_STORAGE_KEY)).toBe("25");
    expect(loadExpiryMinutes(fakeStorage)).toBe(25);

    expect(clampExpiryMinutes(0)).toBe(1);
    expect(clampExpiryMinutes(-10)).toBe(1);
    expect(clampExpiryMinutes(9999)).toBe(1440);
    expect(clampExpiryMinutes(Number.NaN)).toBe(15);
    expect(
      isRowWithinExpiry(
        makeRow(1, "not-a-timestamp"),
        Date.parse("2026-07-02T12:20:00Z"),
        15,
      ),
    ).toBe(true);
  });
});
