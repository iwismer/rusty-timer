import { fireEvent, render, screen } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import AnnouncerTab from "./components/AnnouncerTab.svelte";

const mockState = vi.hoisted(() => ({
  store: {
    announcerEnabled: false,
    announcerBusy: false,
    announcerMaxListSize: 25,
    announcerMaxListBusy: false,
    savedServerUrl: "http://127.0.0.1:8080",
    importBusy: false,
    importMessage: null as string | null,
    importError: null as string | null,
    participantsFilePath: null as string | null,
    chipsFilePath: null as string | null,
    rdImportEnabled: false,
    rdImportDir: "C:\\Winrace\\Files",
    dataStats: null as {
      participants: number;
      chips: number;
      matched_participants: number;
      participants_without_chips: number;
      resolvable_chips: number;
    } | null,
  },
  setAnnouncerEnabled: vi.fn(),
  setAnnouncerMaxListSize: vi.fn(),
  importParticipantsFile: vi.fn(),
  importChipsFile: vi.fn(),
  loadDataStats: vi.fn(),
  openHelp: vi.fn(),
  openFileDialog: vi.fn(),
  openUrl: vi.fn(),
}));

vi.mock("$lib/store.svelte", () => ({
  store: mockState.store,
  setAnnouncerEnabled: mockState.setAnnouncerEnabled,
  setAnnouncerMaxListSize: mockState.setAnnouncerMaxListSize,
  importParticipantsFile: mockState.importParticipantsFile,
  importChipsFile: mockState.importChipsFile,
  loadDataStats: mockState.loadDataStats,
  openHelp: mockState.openHelp,
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: mockState.openFileDialog,
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: mockState.openUrl,
}));

describe("AnnouncerTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockState.store.announcerEnabled = false;
    mockState.store.announcerBusy = false;
    mockState.store.announcerMaxListSize = 25;
    mockState.store.announcerMaxListBusy = false;
    mockState.store.savedServerUrl = "http://127.0.0.1:8080";
    mockState.store.importBusy = false;
    mockState.store.importMessage = null;
    mockState.store.importError = null;
    mockState.store.participantsFilePath = null;
    mockState.store.chipsFilePath = null;
    mockState.store.rdImportEnabled = false;
    mockState.store.rdImportDir = "C:\\Winrace\\Files";
    mockState.store.dataStats = null;
  });

  it("toggling the announcer switch calls setAnnouncerEnabled", async () => {
    render(AnnouncerTab);
    const toggle = screen.getByTestId("announcer-enabled-toggle");
    await fireEvent.click(toggle);
    expect(mockState.setAnnouncerEnabled).toHaveBeenCalledWith(true);
  });

  it("reflects the enabled state from the store", () => {
    mockState.store.announcerEnabled = true;
    render(AnnouncerTab);
    expect(screen.getByTestId("announcer-enabled-toggle")).toBeChecked();
  });

  it("shows an import success message", () => {
    mockState.store.importMessage =
      "Imported 2 participant(s); 2 chip(s) now resolve.";
    render(AnnouncerTab);
    expect(screen.getByTestId("import-message")).toHaveTextContent(
      "Imported 2 participant(s)",
    );
  });

  it("shows an import error message", () => {
    mockState.store.importError =
      "Participant import failed: line 2: invalid bib";
    render(AnnouncerTab);
    expect(screen.getByTestId("import-error")).toHaveTextContent(
      "Participant import failed",
    );
  });

  it("shows selected file paths next to explicit choose buttons", () => {
    mockState.store.participantsFilePath = "C:\\race\\race.ppl";
    mockState.store.chipsFilePath = "C:\\race\\race.bibchip";
    render(AnnouncerTab);

    expect(screen.getByTestId("participants-choose-btn")).toHaveTextContent(
      "Choose file",
    );
    expect(screen.getByTestId("chips-choose-btn")).toHaveTextContent(
      "Choose file",
    );
    expect(screen.getByTestId("participants-file-name")).toHaveTextContent(
      "C:\\race\\race.ppl",
    );
    expect(screen.getByTestId("chips-file-name")).toHaveTextContent(
      "C:\\race\\race.bibchip",
    );
  });

  it("imports participant files with the selected full path", async () => {
    mockState.openFileDialog.mockResolvedValue("C:\\race\\race.ppl");
    render(AnnouncerTab);

    await fireEvent.click(screen.getByTestId("participants-choose-btn"));

    expect(mockState.importParticipantsFile).toHaveBeenCalledWith(
      "C:\\race\\race.ppl",
    );
  });

  it("warns that manual import is not needed when Race Director auto import is enabled", () => {
    mockState.store.rdImportEnabled = true;
    mockState.store.rdImportDir = "D:\\Race\\Files";

    render(AnnouncerTab);

    expect(screen.getByTestId("rd-auto-import-warning")).toHaveTextContent(
      "Race Director auto import is enabled",
    );
    expect(screen.getByTestId("rd-auto-import-warning")).toHaveTextContent(
      "D:\\Race\\Files",
    );
  });

  it("imports chip files with the selected full path", async () => {
    mockState.openFileDialog.mockResolvedValue("C:\\race\\race.bibchip");
    render(AnnouncerTab);

    await fireEvent.click(screen.getByTestId("chips-choose-btn"));

    expect(mockState.importChipsFile).toHaveBeenCalledWith(
      "C:\\race\\race.bibchip",
    );
  });

  it("shows participant and chip data stats", () => {
    mockState.store.dataStats = {
      participants: 100,
      chips: 95,
      matched_participants: 92,
      participants_without_chips: 8,
      resolvable_chips: 92,
    };
    render(AnnouncerTab);

    expect(screen.getByTestId("stat-participants")).toHaveTextContent("100");
    expect(screen.getByTestId("stat-chips")).toHaveTextContent("95");
    expect(screen.getByTestId("stat-matched")).toHaveTextContent("92");
    expect(screen.getByTestId("stat-missing")).toHaveTextContent("8");
    expect(screen.getByTestId("stat-unmatched-chips")).toHaveTextContent("3");
  });

  it("reflects the max list size from the store", () => {
    mockState.store.announcerMaxListSize = 40;
    render(AnnouncerTab);
    expect(screen.getByTestId("announcer-max-list-input")).toHaveValue(40);
  });

  it("opens the server announcer page using the saved server URL", async () => {
    render(AnnouncerTab);
    await fireEvent.click(screen.getByTestId("open-announcer-page-btn"));
    expect(mockState.openUrl).toHaveBeenCalledWith(
      "http://127.0.0.1:8080/announcer",
    );
  });
});
