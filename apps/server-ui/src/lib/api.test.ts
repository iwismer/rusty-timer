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

describe("server api client", () => {
  it("getStatus fetches the public server status", async () => {
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

  it("renameDevice posts JSON to the rename endpoint with the admin header", async () => {
    const { renameDevice } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(200, {
        endpoint_id: "receiver-1",
        device_kind: "receiver",
        display_name: "Finish Line",
        approval_state: "active",
      }),
    );

    const device = await renameDevice("receiver-1", "Finish Line", "alice");

    expect(device.display_name).toBe("Finish Line");
    expect(mockFetch).toHaveBeenCalledWith(
      "/admin/devices/rename",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          "Content-Type": "application/json",
          "Remote-User": "alice",
        }),
        body: JSON.stringify({
          endpoint_id: "receiver-1",
          display_name: "Finish Line",
        }),
      }),
    );
  });

  it("renameDevice rejects blank display names before posting", async () => {
    const { renameDevice } = await import("./api");

    await expect(renameDevice("receiver-1", "   ")).rejects.toThrow(
      "Display name is required",
    );
    expect(mockFetch).not.toHaveBeenCalled();
  });

  it("listEnrollmentTokens fetches token metadata with the admin header", async () => {
    const { listEnrollmentTokens } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(200, {
        tokens: [
          {
            token_id: "et_1",
            device_kind: "forwarder",
            display_name: "Start Line",
            status: "active",
            created_unix_ms: 10,
            used_unix_ms: null,
            used_endpoint_id: null,
            revoked_unix_ms: null,
          },
        ],
      }),
    );

    const response = await listEnrollmentTokens("alice");

    expect(response.tokens[0].token_id).toBe("et_1");
    expect(mockFetch).toHaveBeenCalledWith(
      "/admin/enrollment-tokens",
      expect.objectContaining({
        headers: expect.objectContaining({
          "Content-Type": "application/json",
          "Remote-User": "alice",
        }),
      }),
    );
  });

  it("createEnrollmentToken posts JSON and returns the one-time token", async () => {
    const { createEnrollmentToken } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(200, {
        token_id: "et_1",
        device_kind: "forwarder",
        display_name: "Start Line",
        token: "rtfwd_secret",
        created_unix_ms: 10,
      }),
    );

    const response = await createEnrollmentToken(
      {
        device_kind: "forwarder",
        display_name: "Start Line",
        token: "manual-secret",
      },
      "alice",
    );

    expect(response.token).toBe("rtfwd_secret");
    expect(mockFetch).toHaveBeenCalledWith(
      "/admin/enrollment-tokens",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          "Content-Type": "application/json",
          "Remote-User": "alice",
        }),
        body: JSON.stringify({
          device_kind: "forwarder",
          display_name: "Start Line",
          token: "manual-secret",
        }),
      }),
    );
  });

  it("revokeEnrollmentToken posts to the revoke endpoint", async () => {
    const { revokeEnrollmentToken } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(200, {
        token_id: "et_1",
        device_kind: "forwarder",
        display_name: "Start Line",
        status: "revoked",
        created_unix_ms: 10,
        used_unix_ms: null,
        used_endpoint_id: null,
        revoked_unix_ms: 20,
      }),
    );

    const response = await revokeEnrollmentToken("et_1", "alice");

    expect(response.status).toBe("revoked");
    expect(mockFetch).toHaveBeenCalledWith(
      "/admin/enrollment-tokens/et_1/revoke",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          "Content-Type": "application/json",
          "Remote-User": "alice",
        }),
      }),
    );
  });
});
