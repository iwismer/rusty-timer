import { describe, it, expect, vi, beforeEach } from "vitest";

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

describe("apiFetch", () => {
  it("fetches JSON on success", async () => {
    const { apiFetch } = await import("./api-helpers");
    mockFetch.mockResolvedValue(makeResponse(200, { key: "value" }));
    const result = await apiFetch<{ key: string }>("/api/v1/test");
    expect(result.key).toBe("value");
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/test",
      expect.objectContaining({
        headers: expect.objectContaining({
          "Content-Type": "application/json",
        }),
      }),
    );
  });

  it("throws on non-ok response", async () => {
    const { apiFetch } = await import("./api-helpers");
    mockFetch.mockResolvedValue(makeResponse(500, "internal error"));
    await expect(apiFetch("/api/v1/fail")).rejects.toThrow("500");
  });

  it("returns undefined for 204 No Content", async () => {
    const { apiFetch } = await import("./api-helpers");
    mockFetch.mockResolvedValue(makeResponse(204, null));
    const result = await apiFetch("/api/v1/empty");
    expect(result).toBeUndefined();
  });

  it("passes through custom init options", async () => {
    const { apiFetch } = await import("./api-helpers");
    mockFetch.mockResolvedValue(makeResponse(200, {}));
    await apiFetch("/api/v1/test", { method: "PUT", body: '{"a":1}' });
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/test",
      expect.objectContaining({ method: "PUT", body: '{"a":1}' }),
    );
  });

  it("retains Content-Type when caller passes custom headers", async () => {
    const { apiFetch } = await import("./api-helpers");
    mockFetch.mockResolvedValue(makeResponse(200, { ok: true }));
    await apiFetch("/api/v1/test", {
      method: "POST",
      headers: { "X-Custom": "value" },
      body: '{"cursor":"abc"}',
    });
    const callArgs = mockFetch.mock.calls[0];
    const headers = callArgs[1].headers;
    expect(headers["Content-Type"]).toBe("application/json");
    expect(headers["X-Custom"]).toBe("value");
  });

  it("allows caller to override Content-Type", async () => {
    const { apiFetch } = await import("./api-helpers");
    mockFetch.mockResolvedValue(makeResponse(200, { ok: true }));
    await apiFetch("/api/v1/test", {
      method: "POST",
      headers: { "Content-Type": "text/plain" },
    });
    const callArgs = mockFetch.mock.calls[0];
    expect(callArgs[1].headers["Content-Type"]).toBe("text/plain");
  });
});
