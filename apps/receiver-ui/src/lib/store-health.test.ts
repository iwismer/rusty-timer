import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReaderLiveStatus, UpsStatusPayload } from "./api";

vi.mock("./api", () => ({}));
vi.mock("./desktop-updater", () => ({
  checkForDesktopUpdate: vi.fn(),
  installDesktopUpdate: vi.fn(),
  loadDesktopVersion: vi.fn(),
}));
vi.mock("./sse", () => ({
  destroySSE: vi.fn(),
  initSSE: vi.fn(),
}));
vi.mock("@rusty-timer/shared-ui/lib/dark-mode", () => ({
  cycleTheme: vi.fn(),
}));

type ServerOverrides = Partial<{
  configured: boolean;
  endpoint_id: string | null;
  reachable: boolean | null;
  approval_state: string | null;
  waiting_for_approval: boolean;
  message: string | null;
}>;

type ForwarderOverrides = Partial<{
  endpoint_id: string;
  display_name: string | null;
  state: "subscribed" | "connected" | "unavailable" | "disconnected";
  pending: boolean;
  subscribed_count: number;
  available_count: number;
  readers: ReaderLiveStatus[];
  ups: UpsStatusPayload | null;
  restart_needed: boolean | null;
  remote_config_available: boolean;
}>;

function server(overrides: ServerOverrides = {}) {
  return {
    configured: true,
    endpoint_id: "server-1",
    reachable: true,
    approval_state: "active",
    waiting_for_approval: false,
    message: null,
    ...overrides,
  };
}

function forwarder(overrides: ForwarderOverrides = {}) {
  return {
    endpoint_id: overrides.endpoint_id ?? "fwd-1",
    display_name: null,
    state: "connected" as const,
    pending: false,
    subscribed_count: 1,
    available_count: 1,
    readers: [] as ReaderLiveStatus[],
    ups: null as UpsStatusPayload | null,
    restart_needed: null,
    remote_config_available: false,
    ...overrides,
  };
}

describe("getOverallHealth", () => {
  beforeEach(async () => {
    vi.resetModules();
    const { store } = await import("./store.svelte");
    store.connections = null;
  });

  it("returns ok when the server is approved and all intended forwarders are connected", async () => {
    const { getOverallHealth, store } = await import("./store.svelte");

    store.connections = {
      server: server(),
      forwarders: [
        forwarder({ endpoint_id: "fwd-connected", state: "connected" }),
        forwarder({ endpoint_id: "fwd-subscribed", state: "subscribed" }),
      ],
    };

    expect(getOverallHealth()).toBe("ok");
  });

  it("returns warn for a pending unavailable forwarder during the connect grace", async () => {
    const { getOverallHealth, store } = await import("./store.svelte");

    store.connections = {
      server: server(),
      forwarders: [forwarder({ state: "unavailable", pending: true })],
    };

    expect(getOverallHealth()).toBe("warn");
  });

  it("returns ok when the approved server has no forwarders", async () => {
    const { getOverallHealth, store } = await import("./store.svelte");

    store.connections = {
      server: server(),
      forwarders: [],
    };

    expect(getOverallHealth()).toBe("ok");
  });

  it("returns ok when all forwarders are manually disconnected", async () => {
    const { getOverallHealth, store } = await import("./store.svelte");

    store.connections = {
      server: server(),
      forwarders: [
        forwarder({ endpoint_id: "fwd-1", state: "disconnected" }),
        forwarder({ endpoint_id: "fwd-2", state: "disconnected" }),
      ],
    };

    expect(getOverallHealth()).toBe("ok");
  });

  it("returns err when server approval is inactive and not pending", async () => {
    const { getOverallHealth, store } = await import("./store.svelte");

    store.connections = {
      server: server({
        approval_state: "revoked",
        waiting_for_approval: false,
      }),
      forwarders: [forwarder()],
    };

    expect(getOverallHealth()).toBe("err");
  });

  it("returns err when the configured server is unreachable", async () => {
    const { getOverallHealth, store } = await import("./store.svelte");

    store.connections = {
      server: server({ reachable: false }),
      forwarders: [forwarder()],
    };

    expect(getOverallHealth()).toBe("err");
  });

  it("returns err when all intended forwarders are unavailable", async () => {
    const { getOverallHealth, store } = await import("./store.svelte");

    store.connections = {
      server: server(),
      forwarders: [
        forwarder({ endpoint_id: "fwd-1", state: "unavailable" }),
        forwarder({ endpoint_id: "fwd-2", state: "unavailable" }),
      ],
    };

    expect(getOverallHealth()).toBe("err");
  });

  it("returns warn while server approval is pending", async () => {
    const { getOverallHealth, store } = await import("./store.svelte");

    store.connections = {
      server: server({ approval_state: "pending", waiting_for_approval: true }),
      forwarders: [forwarder()],
    };

    expect(getOverallHealth()).toBe("warn");
  });

  it("returns warn for partial forwarder problems", async () => {
    const { getOverallHealth, store } = await import("./store.svelte");

    store.connections = {
      server: server(),
      forwarders: [
        forwarder({ endpoint_id: "fwd-1", state: "subscribed" }),
        forwarder({ endpoint_id: "fwd-2", state: "unavailable" }),
        forwarder({ endpoint_id: "fwd-3", state: "connected", pending: true }),
      ],
    };

    expect(getOverallHealth()).toBe("warn");
  });

  it("returns warn when server reachability is unknown", async () => {
    const { getOverallHealth, store } = await import("./store.svelte");

    store.connections = {
      server: server({ reachable: null }),
      forwarders: [forwarder()],
    };

    expect(getOverallHealth()).toBe("warn");
  });

  it("returns warn when connection data is unknown or the server is not configured", async () => {
    const { getOverallHealth, store } = await import("./store.svelte");

    store.connections = null;
    expect(getOverallHealth()).toBe("warn");

    store.connections = {
      server: server({
        configured: false,
        reachable: null,
        approval_state: null,
      }),
      forwarders: [],
    };
    expect(getOverallHealth()).toBe("warn");
  });
});
