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
  },
  setAnnouncerEnabled: vi.fn(),
  setAnnouncerMaxListSize: vi.fn(),
  importParticipantsText: vi.fn(),
  importChipsText: vi.fn(),
  openUrl: vi.fn(),
}));

vi.mock("$lib/store.svelte", () => ({
  store: mockState.store,
  setAnnouncerEnabled: mockState.setAnnouncerEnabled,
  setAnnouncerMaxListSize: mockState.setAnnouncerMaxListSize,
  importParticipantsText: mockState.importParticipantsText,
  importChipsText: mockState.importChipsText,
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
