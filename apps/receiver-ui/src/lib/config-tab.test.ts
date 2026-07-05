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
      dbfFlushIntervalMs: 1000,
      editDbfFlushIntervalMs: 1000,
      dbfSaving: false,
      dbfClearing: false,
      editRdImportEnabled: false,
      rdImportEnabled: false,
      editRdImportDir: "C:\\Winrace\\Files",
      rdImportDir: "C:\\Winrace\\Files",
      editRdImportIntervalSecs: 15,
      rdImportIntervalSecs: 15,
      rdImportSaving: false,
      modeDraft: "live",
      modeBusy: false,
    },
    getConfigDirty: vi.fn(() => false),
    getModeDirty: vi.fn(() => false),
    saveProfile: vi.fn(),
    saveDbfConfig: vi.fn(),
    clearDbfFile: vi.fn(),
    saveRdImportConfig: vi.fn(),
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
  saveRdImportConfig: mockState.saveRdImportConfig,
  applyMode: mockState.applyMode,
  markModeEdited: mockState.markModeEdited,
  setModeDraft: mockState.setModeDraft,
  setEditReceiverId: mockState.setEditReceiverId,
  setEditServerUrl: mockState.setEditServerUrl,
  setEditToken: mockState.setEditToken,
}));

vi.mock("@rusty-timer/shared-ui", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@rusty-timer/shared-ui")>()),
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
    mockState.store.dbfFlushIntervalMs = 1000;
    mockState.store.editDbfFlushIntervalMs = 1000;
    mockState.store.dbfSaving = false;
    mockState.store.dbfClearing = false;
    mockState.store.editRdImportEnabled = false;
    mockState.store.rdImportEnabled = false;
    mockState.store.editRdImportDir = "C:\\Winrace\\Files";
    mockState.store.rdImportDir = "C:\\Winrace\\Files";
    mockState.store.editRdImportIntervalSecs = 15;
    mockState.store.rdImportIntervalSecs = 15;
    mockState.store.rdImportSaving = false;
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

  it("renders Race Director import and output config controls", () => {
    render(ConfigTab);

    expect(screen.getByText("Race Director")).toBeInTheDocument();
    expect(screen.getByTestId("rd-import-enabled-toggle")).not.toBeChecked();
    expect(screen.getByTestId("rd-import-dir-input")).toHaveValue(
      "C:\\Winrace\\Files",
    );
    expect(screen.getByTestId("rd-import-interval-input")).toHaveValue(15);
    expect(screen.getByTestId("save-rd-import-btn")).toBeDisabled();

    expect(screen.getByTestId("dbf-enabled-toggle")).not.toBeChecked();
    expect(screen.queryByTestId("dbf-path-input")).not.toBeInTheDocument();
    expect(screen.getByTestId("save-dbf-btn")).toBeDisabled();
    expect(screen.getByTestId("clear-dbf-btn")).toBeEnabled();
    expect(
      screen.queryByTestId("dbf-flush-interval-input"),
    ).not.toBeInTheDocument();
  });

  it("shows the DBF write interval only when DBF output is enabled", () => {
    mockState.store.editDbfEnabled = true;
    mockState.store.editDbfFlushIntervalMs = 1000;

    render(ConfigTab);

    // Rendered in seconds, default 1s.
    expect(screen.getByTestId("dbf-flush-interval-input")).toHaveValue(1);
  });

  it("editing the DBF write interval marks the DBF config dirty and saves", async () => {
    mockState.store.editDbfEnabled = true;
    mockState.store.dbfEnabled = true;
    mockState.store.editDbfFlushIntervalMs = 1000;
    mockState.store.dbfFlushIntervalMs = 1000;

    render(ConfigTab);

    expect(screen.getByTestId("save-dbf-btn")).toBeDisabled();
    await fireEvent.input(screen.getByTestId("dbf-flush-interval-input"), {
      target: { value: "2.5" },
    });
    expect(mockState.store.editDbfFlushIntervalMs).toBe(2500);
    expect(screen.getByTestId("save-dbf-btn")).toBeEnabled();

    await fireEvent.click(screen.getByTestId("save-dbf-btn"));
    expect(mockState.saveDbfConfig).toHaveBeenCalledOnce();
  });

  it("saves Race Director import config separately from DBF output", async () => {
    render(ConfigTab);

    await fireEvent.click(screen.getByTestId("rd-import-enabled-toggle"));
    await fireEvent.input(screen.getByTestId("rd-import-dir-input"), {
      target: { value: "D:\\Race\\Files" },
    });
    await fireEvent.input(screen.getByTestId("rd-import-interval-input"), {
      target: { value: "30" },
    });
    await fireEvent.click(screen.getByTestId("save-rd-import-btn"));

    expect(mockState.saveRdImportConfig).toHaveBeenCalledOnce();
    expect(mockState.saveDbfConfig).not.toHaveBeenCalled();
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
      target: { value: "live" },
    });
    await fireEvent.click(screen.getByTestId("save-mode-btn"));

    expect(mockState.setModeDraft).toHaveBeenCalledWith("live");
    expect(mockState.markModeEdited).toHaveBeenCalledOnce();
    expect(mockState.applyMode).toHaveBeenCalledOnce();
    expect(mockState.saveProfile).not.toHaveBeenCalled();
  });
});
