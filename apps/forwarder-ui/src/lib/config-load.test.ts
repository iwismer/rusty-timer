import { describe, expect, it, vi } from "vitest";
import type { ForwarderConfig, ForwarderStatus } from "./api";
import { loadConfigPageState } from "./config-load";

function makeConfig(): ForwarderConfig {
  return {
    display_name: "Start Line",
    p2p: { enabled: true, server_url: "https://thin.example.com" },
    readers: [{ target: "192.168.1.10:10000", enabled: true }],
  };
}

function makeStatus(restart_needed: boolean): ForwarderStatus {
  return {
    forwarder_id: "fwd-1",
    version: "0.1.0",
    ready: true,
    ready_reason: null,
    p2p_connected: true,
    restart_needed,
    server: {
      configured: false,
      endpoint_id: null,
      reachable: null,
      approval_state: null,
      waiting_for_approval: false,
      message: null,
    },
    readers: [],
  };
}

describe("loadConfigPageState", () => {
  it("sets restartNeeded from status endpoint", async () => {
    const getConfig = vi.fn().mockResolvedValue(makeConfig());
    const getStatus = vi.fn().mockResolvedValue(makeStatus(true));

    const result = await loadConfigPageState(getConfig, getStatus);

    expect(result.loadError).toBeNull();
    expect(result.restartNeeded).toBe(true);
    expect(result.config?.display_name).toBe("Start Line");
  });

  it("keeps config load working when status fetch fails", async () => {
    const getConfig = vi.fn().mockResolvedValue(makeConfig());
    const getStatus = vi.fn().mockRejectedValue(new Error("status down"));

    const result = await loadConfigPageState(getConfig, getStatus);

    expect(result.loadError).toBeNull();
    expect(result.restartNeeded).toBe(false);
    expect(result.config?.display_name).toBe("Start Line");
  });

  it("returns loadError when config fetch fails", async () => {
    const getConfig = vi.fn().mockRejectedValue(new Error("config down"));
    const getStatus = vi.fn().mockResolvedValue(makeStatus(true));

    const result = await loadConfigPageState(getConfig, getStatus);

    expect(result.config).toBeNull();
    expect(result.restartNeeded).toBe(false);
    expect(result.loadError).toContain("config down");
  });
});
