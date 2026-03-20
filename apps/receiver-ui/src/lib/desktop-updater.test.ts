import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  getVersion: vi.fn(),
  check: vi.fn(),
  downloadAndInstall: vi.fn(),
}));

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: mocks.getVersion,
}));

vi.mock("@tauri-apps/plugin-updater", () => ({
  check: mocks.check,
}));

describe("desktop updater", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.clearAllMocks();
    delete (window as Window & { __TAURI_INTERNALS__?: unknown })
      .__TAURI_INTERNALS__;
  });

  it("returns unsupported outside Tauri when loading version", async () => {
    const { loadDesktopVersion } = await import("./desktop-updater");

    await expect(loadDesktopVersion()).resolves.toEqual({
      supported: false,
      version: null,
    });
    expect(mocks.getVersion).not.toHaveBeenCalled();
  });

  it("returns Tauri app version when supported", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ =
      {};
    mocks.getVersion.mockResolvedValue("0.8.0");

    const { loadDesktopVersion } = await import("./desktop-updater");

    await expect(loadDesktopVersion()).resolves.toEqual({
      supported: true,
      version: "0.8.0",
    });
  });

  it("normalizes available update metadata", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ =
      {};
    mocks.getVersion.mockResolvedValue("0.8.0");
    mocks.check.mockResolvedValue({
      version: "0.9.0",
      body: "Fixes receiver update UX",
      date: "2026-03-20T10:00:00Z",
      downloadAndInstall: mocks.downloadAndInstall,
    });

    const { checkForDesktopUpdate } = await import("./desktop-updater");

    await expect(checkForDesktopUpdate()).resolves.toEqual({
      supported: true,
      update: {
        currentVersion: "0.8.0",
        version: "0.9.0",
        notes: "Fixes receiver update UX",
        publishedAt: "2026-03-20T10:00:00Z",
      },
    });
  });

  it("downloads and installs the available update using the cached handle from check", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ =
      {};
    mocks.getVersion.mockResolvedValue("0.8.0");
    mocks.check.mockResolvedValue({
      version: "0.9.0",
      body: null,
      date: null,
      downloadAndInstall: mocks.downloadAndInstall,
    });

    const { checkForDesktopUpdate, installDesktopUpdate } =
      await import("./desktop-updater");

    // check() is called once during checkForDesktopUpdate
    await checkForDesktopUpdate();
    expect(mocks.check).toHaveBeenCalledTimes(1);

    // installDesktopUpdate reuses the cached handle — no second check() call
    await installDesktopUpdate();
    expect(mocks.check).toHaveBeenCalledTimes(1);
    expect(mocks.downloadAndInstall).toHaveBeenCalledTimes(1);
  });
});
