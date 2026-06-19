import { fireEvent, render, screen } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import ConfigTab from "./components/ConfigTab.svelte";

const mockState = vi.hoisted(() => {
  const defaultStatus = () => ({
    connection_state: "disconnected",
    thin_node: {
      configured: true,
      endpoint_id: "node-1",
      reachable: true,
      approval_state: "active",
      waiting_for_approval: false,
      message: null as string | null,
    },
  });

  return {
    defaultStatus,
    store: {
      editReceiverId: "recv-test",
      editThinNodeUrl: "https://thin-node.example.com",
      editToken: "secret",
      savedReceiverId: "recv-test",
      savedThinNodeUrl: "https://thin-node.example.com",
      savedToken: "secret",
      saving: false,
      status: defaultStatus(),
    },
    getConfigDirty: vi.fn(() => false),
    getConnectionState: vi.fn(() => "disconnected"),
    getConnectionBadgeState: vi.fn(() => "err"),
    saveProfile: vi.fn(),
    reconnectThinNode: vi.fn(),
    setEditReceiverId: vi.fn(),
    setEditThinNodeUrl: vi.fn(),
    setEditToken: vi.fn(),
  };
});

vi.mock("$lib/store.svelte", () => ({
  store: mockState.store,
  getConfigDirty: mockState.getConfigDirty,
  getConnectionState: mockState.getConnectionState,
  getConnectionBadgeState: mockState.getConnectionBadgeState,
  saveProfile: mockState.saveProfile,
  reconnectThinNode: mockState.reconnectThinNode,
  setEditReceiverId: mockState.setEditReceiverId,
  setEditThinNodeUrl: mockState.setEditThinNodeUrl,
  setEditToken: mockState.setEditToken,
}));

vi.mock("@rusty-timer/shared-ui", () => ({
  HelpTip: () => null,
}));

describe("ConfigTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockState.store.editReceiverId = "recv-test";
    mockState.store.editThinNodeUrl = "https://thin-node.example.com";
    mockState.store.editToken = "secret";
    mockState.store.savedReceiverId = "recv-test";
    mockState.store.savedThinNodeUrl = "https://thin-node.example.com";
    mockState.store.savedToken = "secret";
    mockState.store.saving = false;
    mockState.store.status = mockState.defaultStatus();
    mockState.getConfigDirty.mockReturnValue(false);
    mockState.getConnectionState.mockReturnValue("disconnected");
    mockState.getConnectionBadgeState.mockReturnValue("err");
  });

  it("renders config inputs and the current connection state", () => {
    render(ConfigTab);

    expect(screen.getByTestId("receiver-id-input")).toHaveValue("recv-test");
    expect(screen.getByTestId("thin-node-url-input")).toHaveValue(
      "https://thin-node.example.com",
    );
    expect(screen.getByTestId("token-input")).toHaveValue("secret");
    expect(screen.getByTestId("save-config-btn")).toBeDisabled();
    expect(screen.getByTestId("config-connection-state")).toHaveTextContent(
      "Disconnected",
    );
  });

  it("shows when the receiver is waiting for thin-node approval", () => {
    mockState.store.status.thin_node = {
      configured: true,
      endpoint_id: "node-1",
      reachable: true,
      approval_state: "pending",
      waiting_for_approval: true,
      message: "Waiting for thin-node admin approval",
    };

    render(ConfigTab);

    expect(screen.getByTestId("thin-node-approval-state")).toHaveTextContent(
      "Waiting for thin-node admin approval",
    );
  });

  it("offers a manual thin-node reconnect action", async () => {
    render(ConfigTab);

    await fireEvent.click(screen.getByTestId("reconnect-thin-node-btn"));

    expect(mockState.reconnectThinNode).toHaveBeenCalledOnce();
  });

  it("reflects the connected state driven by the backend", () => {
    mockState.store.status = {
      ...mockState.defaultStatus(),
      connection_state: "connected",
    };
    mockState.getConnectionState.mockReturnValue("connected");
    mockState.getConnectionBadgeState.mockReturnValue("ok");

    render(ConfigTab);

    expect(screen.getByTestId("config-connection-state")).toHaveTextContent(
      "Connected",
    );
  });

  it("reflects the connecting transitional state", () => {
    mockState.store.status = {
      ...mockState.defaultStatus(),
      connection_state: "connecting",
    };
    mockState.getConnectionState.mockReturnValue("connecting");
    mockState.getConnectionBadgeState.mockReturnValue("warn");

    render(ConfigTab);

    expect(screen.getByTestId("config-connection-state")).toHaveTextContent(
      "Connecting...",
    );
  });
});
