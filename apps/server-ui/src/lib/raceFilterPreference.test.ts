import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  readRaceFilterPreference,
  writeRaceFilterPreference,
} from "./raceFilterPreference";

describe("raceFilterPreference", () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
  });

  it("returns null when localStorage is unavailable", () => {
    expect(readRaceFilterPreference()).toBe(null);
  });

  it("reads stored race ID from localStorage", () => {
    const getItem = vi.fn(() => "abc-123");
    const localStorageMock = { getItem } as unknown as Storage;
    vi.stubGlobal("localStorage", localStorageMock);

    expect(readRaceFilterPreference()).toBe("abc-123");
    expect(getItem).toHaveBeenCalledWith("raceFilter");
  });

  it("returns null when localStorage read throws", () => {
    const localStorageMock = {
      getItem: vi.fn(() => {
        throw new Error("blocked");
      }),
    } as unknown as Storage;
    vi.stubGlobal("localStorage", localStorageMock);

    expect(readRaceFilterPreference()).toBe(null);
  });

  it("writes race ID when localStorage is available", () => {
    const setItem = vi.fn();
    const localStorageMock = { setItem } as unknown as Storage;
    vi.stubGlobal("localStorage", localStorageMock);

    writeRaceFilterPreference("abc-123");

    expect(setItem).toHaveBeenCalledWith("raceFilter", "abc-123");
  });

  it("removes key when writing null", () => {
    const removeItem = vi.fn();
    const localStorageMock = { removeItem } as unknown as Storage;
    vi.stubGlobal("localStorage", localStorageMock);

    writeRaceFilterPreference(null);

    expect(removeItem).toHaveBeenCalledWith("raceFilter");
  });

  it("does not throw when localStorage write throws", () => {
    const localStorageMock = {
      setItem: vi.fn(() => {
        throw new Error("blocked");
      }),
    } as unknown as Storage;
    vi.stubGlobal("localStorage", localStorageMock);

    expect(() => writeRaceFilterPreference("abc-123")).not.toThrow();
  });
});
