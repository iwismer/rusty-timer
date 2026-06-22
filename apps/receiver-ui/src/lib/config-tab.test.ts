import { fireEvent, render, screen } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import ConfigTab from "./components/ConfigTab.svelte";

const mockState = vi.hoisted(() => {
  const defaultStatus = () => ({
    connection_state: "disconnected",
    server: {
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
      editServerUrl: "https://server.example.com",
      editToken: "secret",
      savedReceiverId: "recv-test",
      savedServerUrl: "https://server.example.com",
      savedToken: "secret",
      serverSource: "profile",
      saving: false,
      status: defaultStatus(),
      editDbfEnabled: false,
      dbfEnabled: false,
      editDbfPath: "C:\\winrace\\Files\\IPICO.DBF",
      dbfPath: "C:\\winrace\\Files\\IPICO.DBF",
      dbfSaving: false,
      dbfClearing: false,
      modeDraft: "live",
      modeBusy: false,
    },
    getConfigDirty: vi.fn(() => false),
    getModeDirty: vi.fn(() => false),
    saveProfile: vi.fn(),
    saveDbfConfig: vi.fn(),
    clearDbfFile: vi.fn(),
    applyMode: vi.fn(),
    markModeEdited: vi.fn(),
    setModeDraft: vi.fn(),
    setEditReceiverId: vi.fn(),
    setEditServerUrl: vi.fn(),
    setEditToken: vi.fn(),
  };
});

vi.mock("$lib/store.svelte", () => ({
  store: mockState.store,
  getConfigDirty: mockState.getConfigDirty,
  getModeDirty: mockState.getModeDirty,
  saveProfile: mockState.saveProfile,
  saveDbfConfig: mockState.saveDbfConfig,
  clearDbfFile: mockState.clearDbfFile,
  applyMode: mockState.applyMode,
  markModeEdited: mockState.markModeEdited,
  setModeDraft: mockState.setModeDraft,
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
    mockState.store.editServerUrl = "https://server.example.com";
    mockState.store.editToken = "secret";
    mockState.store.savedReceiverId = "recv-test";
    mockState.store.savedServerUrl = "https://server.example.com";
    mockState.store.savedToken = "secret";
    mockState.store.serverSource = "profile";
    mockState.store.saving = false;
    mockState.store.status = mockState.defaultStatus();
    mockState.store.editDbfEnabled = false;
    mockState.store.dbfEnabled = false;
    mockState.store.editDbfPath = "C:\\winrace\\Files\\IPICO.DBF";
    mockState.store.dbfPath = "C:\\winrace\\Files\\IPICO.DBF";
    mockState.store.dbfSaving = false;
    mockState.store.dbfClearing = false;
    mockState.store.modeDraft = "live";
    mockState.store.modeBusy = false;
    mockState.getConfigDirty.mockReturnValue(false);
    mockState.getModeDirty.mockReturnValue(false);
  });

  it("renders server config inputs without connection status", () => {
    mockState.store.status.server = {
      configured: true,
      endpoint_id: "node-1",
      reachable: true,
      approval_state: "pending",
      waiting_for_approval: true,
      message: "Waiting for server admin approval",
    };

    render(ConfigTab);

    expect(screen.getByTestId("receiver-id-input")).toHaveValue("recv-test");
    expect(screen.getByTestId("server-url-input")).toHaveValue(
      "https://server.example.com",
    );
    expect(screen.getByTestId("token-input")).toHaveValue("secret");
    expect(screen.getByTestId("save-config-btn")).toBeDisabled();
    expect(
      screen.queryByTestId("config-connection-state"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByTestId("server-approval-state"),
    ).not.toBeInTheDocument();
  });

  it("locks the server fields and shows a note when overridden by env", () => {
    mockState.store.serverSource = "env";

    render(ConfigTab);

    expect(screen.getByTestId("server-url-input")).toBeDisabled();
    expect(screen.getByTestId("token-input")).toBeDisabled();
    expect(screen.getByTestId("server-env-override-note")).toBeInTheDocument();
    // The receiver ID stays editable.
    expect(screen.getByTestId("receiver-id-input")).not.toBeDisabled();
  });

  it("keeps the server fields editable when sourced from the profile", () => {
    render(ConfigTab);

    expect(screen.getByTestId("server-url-input")).not.toBeDisabled();
    expect(screen.getByTestId("token-input")).not.toBeDisabled();
    expect(
      screen.queryByTestId("server-env-override-note"),
    ).not.toBeInTheDocument();
  });

  it("enables saving when the server config is dirty", () => {
    mockState.getConfigDirty.mockReturnValue(true);

    render(ConfigTab);

    expect(screen.getByTestId("save-config-btn")).toBeEnabled();
  });

  it("omits server connection controls because status lives in Connections", () => {
    render(ConfigTab);

    expect(screen.queryByText("Connection")).not.toBeInTheDocument();
    expect(
      screen.queryByTestId("reconnect-server-btn"),
    ).not.toBeInTheDocument();
  });

  it("renders Race Director output config controls", () => {
    render(ConfigTab);

    expect(screen.getByTestId("dbf-enabled-toggle")).not.toBeChecked();
    expect(screen.getByTestId("dbf-path-input")).toHaveValue(
      "C:\\winrace\\Files\\IPICO.DBF",
    );
    expect(screen.getByTestId("save-dbf-btn")).toBeDisabled();
    expect(screen.getByTestId("clear-dbf-btn")).toBeEnabled();
  });

  it("renders receiver mode controls with a separate apply button", () => {
    mockState.getModeDirty.mockReturnValue(true);

    render(ConfigTab);

    expect(screen.getByTestId("mode-select")).toHaveValue("live");
    expect(screen.getByTestId("save-mode-btn")).toBeEnabled();
    expect(screen.getByTestId("save-config-btn")).toBeDisabled();
  });

  it("tracks receiver mode edits separately from profile config", async () => {
    mockState.getModeDirty.mockReturnValue(true);

    render(ConfigTab);

    await fireEvent.change(screen.getByTestId("mode-select"), {
      target: { value: "targeted_replay" },
    });
    await fireEvent.click(screen.getByTestId("save-mode-btn"));

    expect(mockState.setModeDraft).toHaveBeenCalledWith("targeted_replay");
    expect(mockState.markModeEdited).toHaveBeenCalledOnce();
    expect(mockState.applyMode).toHaveBeenCalledOnce();
    expect(mockState.saveProfile).not.toHaveBeenCalled();
  });
});
