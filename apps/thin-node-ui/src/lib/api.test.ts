import { beforeEach, describe, expect, it, vi } from "vitest";

const mockFetch = vi.fn();
vi.stubGlobal("fetch", mockFetch);

beforeEach(() => {
  mockFetch.mockReset();
});

function makeResponse(status: number, body: unknown) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
    text: async () => JSON.stringify(body),
  };
}

describe("thin-node api client", () => {
  it("getStatus fetches the public thin-node status", async () => {
    const { getStatus } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(200, {
        announcer_source_generation: 3,
        finisher_count: 12,
        announcer_rows: [],
        devices: [
          {
            endpoint_id: "fwd-1",
            device_kind: "forwarder",
            display_name: "Start Line",
            approval_state: "active",
          },
        ],
        forwarder_streams: [],
      }),
    );

    const status = await getStatus();

    expect(status.announcer_source_generation).toBe(3);
    expect(status.devices[0].device_kind).toBe("forwarder");
    expect(status.devices[0].approval_state).toBe("active");
    expect(mockFetch).toHaveBeenCalledWith("/status", expect.any(Object));
  });

  it("approveDevice posts JSON with the Remote-User admin header", async () => {
    const { approveDevice } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(200, {
        endpoint_id: "receiver-1",
        device_kind: "receiver",
        display_name: "Finish Tablet",
        approval_state: "active",
      }),
    );

    const device = await approveDevice("receiver-1", "Finish Tablet", "alice");

    expect(device.approval_state).toBe("active");
    expect(mockFetch).toHaveBeenCalledWith(
      "/admin/devices/approve",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          "Content-Type": "application/json",
          "Remote-User": "alice",
        }),
        body: JSON.stringify({
          endpoint_id: "receiver-1",
          display_name: "Finish Tablet",
        }),
      }),
    );
  });

  it("approveDevice defaults the Remote-User header to dev-admin", async () => {
    const { approveDevice } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(200, {
        endpoint_id: "receiver-1",
        device_kind: "receiver",
        display_name: "Finish Tablet",
        approval_state: "active",
      }),
    );

    await approveDevice("receiver-1", "Finish Tablet");

    expect(mockFetch).toHaveBeenCalledWith(
      "/admin/devices/approve",
      expect.objectContaining({
        headers: expect.objectContaining({ "Remote-User": "dev-admin" }),
      }),
    );
  });

  it("approveDevice rejects blank display names before posting", async () => {
    const { approveDevice } = await import("./api");

    await expect(approveDevice("receiver-1", "   ")).rejects.toThrow(
      "Display name is required",
    );
    expect(mockFetch).not.toHaveBeenCalled();
  });
});
