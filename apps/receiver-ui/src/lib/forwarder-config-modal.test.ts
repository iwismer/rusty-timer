import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import ForwarderConfigModal from "./components/ForwarderConfigModal.svelte";

const mockApi = vi.hoisted(() => ({
  getForwarderConfig: vi.fn(),
  setForwarderConfig: vi.fn(),
  restartForwarder: vi.fn(),
}));

vi.mock("$lib/api", () => ({
  getForwarderConfig: mockApi.getForwarderConfig,
  setForwarderConfig: mockApi.setForwarderConfig,
  restartForwarder: mockApi.restartForwarder,
}));

const sampleConfig = () => ({
  schema_version: 1,
  display_name: "Old Forwarder",
  p2p: {
    enabled: true,
    server_url: "https://server.example.com",
    server_token_file: "/etc/rusty/server.token",
  },
  auth: {
    token_file: "/etc/rusty/auth.token",
  },
  journal: {
    sqlite_path: "/var/lib/rusty/forwarder.db",
    prune_watermark_pct: 80,
  },
  status_http: {
    bind: "0.0.0.0:8080",
  },
  control: {
    allow_power_actions: false,
    allow_remote_config: true,
  },
  update: {
    mode: "check-only",
  },
  readers: [
    {
      target: "192.168.0.50:10000",
      enabled: true,
      local_fallback_port: 10050,
      untouched_reader_field: "keep-reader-field",
    },
  ],
  untouched_top_level: {
    nested: "keep-me",
  },
});

describe("ForwarderConfigModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockApi.getForwarderConfig.mockResolvedValue({
      config_json: JSON.stringify(sampleConfig()),
      restart_needed: false,
    });
    mockApi.setForwarderConfig.mockResolvedValue({
      ok: true,
      restart_needed: false,
      error: "",
    });
    mockApi.restartForwarder.mockResolvedValue({ accepted: true, error: "" });
  });

  it("loads config and saves the mutated full document with untouched fields preserved", async () => {
    render(ForwarderConfigModal, {
      open: true,
      endpointId: "endpoint-1",
      displayName: "Timing Forwarder",
      onClose: vi.fn(),
    });

    const displayNameInput = await screen.findByLabelText("Display name");
    await fireEvent.input(displayNameInput, {
      target: { value: "New Forwarder" },
    });
    await fireEvent.click(screen.getByTestId("forwarder-config-save"));

    await waitFor(() => {
      expect(mockApi.setForwarderConfig).toHaveBeenCalledWith(
        "endpoint-1",
        expect.any(String),
      );
    });

    const submitted = JSON.parse(mockApi.setForwarderConfig.mock.calls[0][1]);
    expect(submitted.display_name).toBe("New Forwarder");
    expect(submitted.untouched_top_level).toEqual({ nested: "keep-me" });
    expect(submitted.auth.token_file).toBe("/etc/rusty/auth.token");
    expect(submitted.readers[0].untouched_reader_field).toBe(
      "keep-reader-field",
    );
    // A real update mode is preserved (not stripped).
    expect(submitted.update.mode).toBe("check-only");
  });

  it("omits an empty (Default) update.mode so the forwarder keeps its default", async () => {
    // Config without an [update] section: the modal synthesizes an empty one and
    // the "Default" select option binds mode to "". Saving must NOT send
    // update.mode === "" (the forwarder rejects an empty mode).
    const noUpdate = sampleConfig() as Record<string, any>;
    delete noUpdate.update;
    mockApi.getForwarderConfig.mockResolvedValue({
      config_json: JSON.stringify(noUpdate),
      restart_needed: false,
    });

    render(ForwarderConfigModal, {
      open: true,
      endpointId: "endpoint-1",
      displayName: "Timing Forwarder",
      onClose: vi.fn(),
    });

    await screen.findByLabelText("Display name");
    await fireEvent.click(screen.getByTestId("forwarder-config-save"));

    await waitFor(() => {
      expect(mockApi.setForwarderConfig).toHaveBeenCalled();
    });

    const submitted = JSON.parse(mockApi.setForwarderConfig.mock.calls[0][1]);
    // Other fields still preserved (full-document round-trip).
    expect(submitted.untouched_top_level).toEqual({ nested: "keep-me" });
    // The empty mode must be omitted entirely, not sent as "".
    expect(submitted.update?.mode).toBeUndefined();
  });

  it("renders protected sections read-only and round-trips their values verbatim", async () => {
    render(ForwarderConfigModal, {
      open: true,
      endpointId: "endpoint-1",
      displayName: "Timing Forwarder",
      onClose: vi.fn(),
    });

    await screen.findByLabelText("Display name");

    // Protected [p2p] and [control] inputs must be disabled.
    expect(screen.getByLabelText("Enable P2P")).toBeDisabled();
    expect(screen.getByLabelText("Server URL")).toBeDisabled();
    expect(screen.getByLabelText("Server token file")).toBeDisabled();
    expect(screen.getByLabelText("Allow power actions")).toBeDisabled();
    expect(screen.getByLabelText("Allow remote config")).toBeDisabled();

    // Each protected card explains why the fields are read-only.
    expect(
      screen.getAllByText(
        "Managed locally on the forwarder — not editable remotely.",
      ),
    ).toHaveLength(2);

    // Operational fields stay editable.
    expect(screen.getByLabelText("Display name")).toBeEnabled();
    expect(screen.getByLabelText("SQLite path")).toBeEnabled();

    await fireEvent.click(screen.getByTestId("forwarder-config-save"));

    await waitFor(() => {
      expect(mockApi.setForwarderConfig).toHaveBeenCalled();
    });

    // Protected sections must round-trip unchanged — the forwarder rejects
    // any write whose [auth]/[p2p]/[control] differ from the on-disk config.
    const submitted = JSON.parse(mockApi.setForwarderConfig.mock.calls[0][1]);
    const fetched = sampleConfig();
    expect(submitted.p2p).toEqual(fetched.p2p);
    expect(submitted.control).toEqual(fetched.control);
    expect(submitted.auth).toEqual(fetched.auth);
  });

  it("shows a restart banner after a save that requires restart and restarts on demand", async () => {
    mockApi.setForwarderConfig.mockResolvedValue({
      ok: true,
      restart_needed: true,
      error: "",
    });

    render(ForwarderConfigModal, {
      open: true,
      endpointId: "endpoint-1",
      displayName: "Timing Forwarder",
      onClose: vi.fn(),
    });

    await screen.findByLabelText("Display name");
    await fireEvent.click(screen.getByTestId("forwarder-config-save"));

    expect(
      await screen.findByTestId("forwarder-config-restart-banner"),
    ).toBeInTheDocument();

    await fireEvent.click(screen.getByTestId("forwarder-config-restart"));

    expect(mockApi.restartForwarder).toHaveBeenCalledWith("endpoint-1");
  });

  it("shows the returned save error", async () => {
    mockApi.setForwarderConfig.mockResolvedValue({
      ok: false,
      restart_needed: false,
      error: "remote config disabled",
    });

    render(ForwarderConfigModal, {
      open: true,
      endpointId: "endpoint-1",
      displayName: "Timing Forwarder",
      onClose: vi.fn(),
    });

    await screen.findByLabelText("Display name");
    await fireEvent.click(screen.getByTestId("forwarder-config-save"));

    expect(
      await screen.findByTestId("forwarder-config-error"),
    ).toHaveTextContent("remote config disabled");
  });
});
