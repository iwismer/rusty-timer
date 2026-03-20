import { fireEvent, render, screen } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import ConfigTab from "./components/ConfigTab.svelte";

const mockState = vi.hoisted(() => ({
  store: {
    editReceiverId: "recv-test",
    editServerUrl: "wss://server.example/ws",
    editToken: "secret",
    savedReceiverId: "recv-test",
    savedServerUrl: "wss://server.example/ws",
    savedToken: "secret",
    saving: false,
    connectBusy: false,
    status: { connection_state: "disconnected" },
  },
  getConfigDirty: vi.fn(() => false),
  getConnectionState: vi.fn(() => "disconnected"),
  saveProfile: vi.fn(),
  handleConnect: vi.fn(),
  handleDisconnect: vi.fn(),
  setEditReceiverId: vi.fn(),
  setEditServerUrl: vi.fn(),
  setEditToken: vi.fn(),
}));

vi.mock("$lib/store.svelte", () => ({
  store: mockState.store,
  getConfigDirty: mockState.getConfigDirty,
  getConnectionState: mockState.getConnectionState,
  saveProfile: mockState.saveProfile,
  handleConnect: mockState.handleConnect,
  handleDisconnect: mockState.handleDisconnect,
  setEditReceiverId: mockState.setEditReceiverId,
  setEditServerUrl: mockState.setEditServerUrl,
  setEditToken: mockState.setEditToken,
}));

vi.mock("@rusty-timer/shared-ui", () => ({
  HelpTip: () => null,
}));

describe("ConfigTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockState.store.editReceiverId = "recv-test";
    mockState.store.editServerUrl = "wss://server.example/ws";
    mockState.store.editToken = "secret";
    mockState.store.savedReceiverId = "recv-test";
    mockState.store.savedServerUrl = "wss://server.example/ws";
    mockState.store.savedToken = "secret";
    mockState.store.saving = false;
    mockState.store.connectBusy = false;
    mockState.store.status = { connection_state: "disconnected" };
    mockState.getConfigDirty.mockReturnValue(false);
    mockState.getConnectionState.mockReturnValue("disconnected");
  });

  it("renders config inputs and the current connection state", () => {
    render(ConfigTab);

    expect(screen.getByTestId("receiver-id-input")).toHaveValue("recv-test");
    expect(screen.getByTestId("server-url-input")).toHaveValue(
      "wss://server.example/ws",
    );
    expect(screen.getByTestId("token-input")).toHaveValue("secret");
    expect(screen.getByTestId("save-config-btn")).toBeDisabled();
    expect(screen.getByTestId("config-connection-state")).toHaveTextContent(
      "Disconnected",
    );
    expect(screen.getByRole("button", { name: "Connect" })).toBeInTheDocument();
  });

  it("calls handleConnect when the config tab connect button is pressed", async () => {
    render(ConfigTab);

    await fireEvent.click(screen.getByRole("button", { name: "Connect" }));

    expect(mockState.handleConnect).toHaveBeenCalledTimes(1);
  });

  it("shows disconnect when currently connected", async () => {
    mockState.store.status = { connection_state: "connected" };
    mockState.getConnectionState.mockReturnValue("connected");

    render(ConfigTab);

    const button = screen.getByRole("button", { name: "Disconnect" });
    await fireEvent.click(button);

    expect(screen.getByTestId("config-connection-state")).toHaveTextContent(
      "Connected",
    );
    expect(mockState.handleDisconnect).toHaveBeenCalledTimes(1);
  });

  it("disables connect when there is no saved server URL", () => {
    mockState.store.editServerUrl = "wss://draft-only.example/ws";
    mockState.store.savedServerUrl = "";

    render(ConfigTab);

    expect(screen.getByRole("button", { name: "Connect" })).toBeDisabled();
  });

  it("renders connecting as a disabled transitional state", () => {
    mockState.store.status = { connection_state: "connecting" };
    mockState.getConnectionState.mockReturnValue("connecting");

    render(ConfigTab);

    expect(
      screen.getByRole("button", { name: "Connecting..." }),
    ).toBeDisabled();
    expect(screen.getByTestId("config-connection-state")).toHaveTextContent(
      "Connecting...",
    );
  });
});
