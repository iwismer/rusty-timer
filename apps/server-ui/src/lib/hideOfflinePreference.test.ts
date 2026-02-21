import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  readHideOfflinePreference,
  writeHideOfflinePreference,
} from "./hideOfflinePreference";

describe("hideOfflinePreference", () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
  });

  it("returns false when localStorage is unavailable", () => {
    expect(readHideOfflinePreference()).toBe(false);
  });

  it("reads true when localStorage has true value", () => {
    const getItem = vi.fn(() => "true");
    const localStorageMock = { getItem } as unknown as Storage;
    vi.stubGlobal("localStorage", localStorageMock);

    expect(readHideOfflinePreference()).toBe(true);
    expect(getItem).toHaveBeenCalledWith("hideOffline");
  });

  it("returns false when localStorage read throws", () => {
    const localStorageMock = {
      getItem: vi.fn(() => {
        throw new Error("blocked");
      }),
    } as unknown as Storage;
    vi.stubGlobal("localStorage", localStorageMock);

    expect(readHideOfflinePreference()).toBe(false);
  });

  it("writes preference when localStorage is available", () => {
    const setItem = vi.fn();
    const localStorageMock = { setItem } as unknown as Storage;
    vi.stubGlobal("localStorage", localStorageMock);

    writeHideOfflinePreference(true);

    expect(setItem).toHaveBeenCalledWith("hideOffline", "true");
  });

  it("does not throw when localStorage write throws", () => {
    const localStorageMock = {
      setItem: vi.fn(() => {
        throw new Error("blocked");
      }),
    } as unknown as Storage;
    vi.stubGlobal("localStorage", localStorageMock);

    expect(() => writeHideOfflinePreference(false)).not.toThrow();
  });
});
