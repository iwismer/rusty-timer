import { render, screen } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import ConfigTab from "./components/ConfigTab.svelte";

const mockState = vi.hoisted(() => ({
  store: {
    editReceiverId: "recv-test",
    editThinNodeUrl: "https://thin-node.example.com",
    editToken: "secret",
    savedReceiverId: "recv-test",
    savedThinNodeUrl: "https://thin-node.example.com",
    savedToken: "secret",
    saving: false,
    status: { connection_state: "disconnected" },
  },
  getConfigDirty: vi.fn(() => false),
  getConnectionState: vi.fn(() => "disconnected"),
  getConnectionBadgeState: vi.fn(() => "err"),
  saveProfile: vi.fn(),
  setEditReceiverId: vi.fn(),
  setEditThinNodeUrl: vi.fn(),
  setEditToken: vi.fn(),
}));

vi.mock("$lib/store.svelte", () => ({
  store: mockState.store,
  getConfigDirty: mockState.getConfigDirty,
  getConnectionState: mockState.getConnectionState,
  getConnectionBadgeState: mockState.getConnectionBadgeState,
  saveProfile: mockState.saveProfile,
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
    mockState.store.status = { connection_state: "disconnected" };
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

  it("shows a read-only connection indicator with no manual controls", () => {
    render(ConfigTab);

    // The connection state is read-only: there are no Connect/Disconnect
    // buttons (the P2P session drives the state automatically).
    expect(
      screen.queryByRole("button", { name: "Connect" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Disconnect" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByTestId("config-connect-toggle-btn"),
    ).not.toBeInTheDocument();
  });

  it("reflects the connected state driven by the backend", () => {
    mockState.store.status = { connection_state: "connected" };
    mockState.getConnectionState.mockReturnValue("connected");
    mockState.getConnectionBadgeState.mockReturnValue("ok");

    render(ConfigTab);

    expect(screen.getByTestId("config-connection-state")).toHaveTextContent(
      "Connected",
    );
  });

  it("reflects the connecting transitional state", () => {
    mockState.store.status = { connection_state: "connecting" };
    mockState.getConnectionState.mockReturnValue("connecting");
    mockState.getConnectionBadgeState.mockReturnValue("warn");

    render(ConfigTab);

    expect(screen.getByTestId("config-connection-state")).toHaveTextContent(
      "Connecting...",
    );
  });
});
