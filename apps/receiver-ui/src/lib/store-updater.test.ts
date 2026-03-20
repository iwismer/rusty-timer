import { beforeEach, describe, expect, it, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  getStatus: vi.fn().mockResolvedValue({
    connection_state: "disconnected",
    local_ok: true,
    streams_count: 0,
    receiver_id: "recv-test",
  }),
  getStreams: vi.fn().mockResolvedValue({
    streams: [],
    degraded: false,
    upstream_error: null,
  }),
  getLogs: vi.fn().mockResolvedValue({ entries: [] }),
  getProfile: vi.fn().mockResolvedValue(null),
  getUpdateStatus: vi.fn().mockResolvedValue(null),
  getMode: vi.fn().mockResolvedValue({
    mode: "live",
    streams: [],
    earliest_epochs: [],
  }),
  getRaces: vi.fn().mockResolvedValue({ races: [] }),
  getReplayTargetEpochs: vi.fn().mockResolvedValue({ epochs: [] }),
  checkForUpdate: vi.fn().mockResolvedValue({ status: "up_to_date" }),
  downloadUpdate: vi.fn().mockResolvedValue({ status: "downloaded" }),
  applyUpdate: vi.fn().mockResolvedValue(undefined),
}));

const desktopUpdaterMocks = vi.hoisted(() => ({
  loadDesktopVersion: vi.fn().mockResolvedValue({
    supported: true,
    version: "0.8.0",
  }),
  checkForDesktopUpdate: vi.fn().mockResolvedValue({
    supported: true,
    update: {
      currentVersion: "0.8.0",
      version: "0.9.0",
      notes: "Receiver release notes",
      publishedAt: "2026-03-20T10:00:00Z",
    },
  }),
  installDesktopUpdate: vi.fn().mockResolvedValue(undefined),
}));

const eventMocks = vi.hoisted(() => {
  const listeners = new Map<string, () => void>();
  return {
    listeners,
    listen: vi.fn(
      async (eventName: string, callback: () => void): Promise<() => void> => {
        listeners.set(eventName, callback);
        return () => {
          listeners.delete(eventName);
        };
      },
    ),
  };
});

const sseMocks = vi.hoisted(() => ({
  initSSE: vi.fn(),
  destroySSE: vi.fn(),
}));

const darkModeMocks = vi.hoisted(() => ({
  cycleTheme: vi.fn(),
}));

const mockFetch = vi.hoisted(() => vi.fn());

vi.mock("./api", () => apiMocks);
vi.mock("./desktop-updater", () => desktopUpdaterMocks);
vi.mock("./sse", () => sseMocks);
vi.mock("@tauri-apps/api/event", () => ({
  listen: eventMocks.listen,
}));
vi.mock("@rusty-timer/shared-ui/lib/dark-mode", () => darkModeMocks);

vi.stubGlobal("fetch", mockFetch);

async function flushAsyncWork(): Promise<void> {
  await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe("receiver updater store", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.clearAllMocks();
    eventMocks.listeners.clear();
    mockFetch.mockResolvedValue({
      json: async () => ({ version: "legacy-version" }),
    });
  });

  it("loads the app version from the desktop updater instead of the receiver version endpoint", async () => {
    const { initStore, store } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    expect(desktopUpdaterMocks.loadDesktopVersion).toHaveBeenCalledTimes(1);
    expect(store.appVersion).toBe("0.8.0");
    expect(mockFetch).not.toHaveBeenCalled();
  });

  it("checks for updates through Tauri when the menu event fires and opens the modal", async () => {
    const { initStore, store } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const onCheckUpdate = eventMocks.listeners.get("menu-check-update");
    expect(onCheckUpdate).toBeTypeOf("function");
    desktopUpdaterMocks.checkForDesktopUpdate.mockClear();

    onCheckUpdate?.();
    await flushAsyncWork();

    expect(desktopUpdaterMocks.checkForDesktopUpdate).toHaveBeenCalledTimes(1);
    expect(apiMocks.checkForUpdate).not.toHaveBeenCalled();
    expect(store.updateModalOpen).toBe(true);
    expect(store.updateState?.notes).toBe("Receiver release notes");
  });

  it("installs through the desktop updater instead of receiver download/apply endpoints", async () => {
    const { confirmUpdateInstall, store } = await import("./store.svelte");

    store.updateState = {
      status: "available",
      currentVersion: "0.8.0",
      version: "0.9.0",
      notes: null,
      busy: false,
      error: null,
    };

    await confirmUpdateInstall();

    expect(desktopUpdaterMocks.installDesktopUpdate).toHaveBeenCalledTimes(1);
    expect(apiMocks.downloadUpdate).not.toHaveBeenCalled();
    expect(apiMocks.applyUpdate).not.toHaveBeenCalled();
  });

  it("hydrates config edit fields from the saved profile on initial load", async () => {
    apiMocks.getProfile.mockResolvedValueOnce({
      server_url: "wss://receiver.example/ws",
      token: "secret-token",
      receiver_id: "recv-live",
    });

    const { initStore, store } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    expect(store.editServerUrl).toBe("wss://receiver.example/ws");
    expect(store.editToken).toBe("secret-token");
    expect(store.editReceiverId).toBe("recv-live");
    expect(store.savedServerUrl).toBe("wss://receiver.example/ws");
    expect(store.savedToken).toBe("secret-token");
    expect(store.savedReceiverId).toBe("recv-live");
  });
});
