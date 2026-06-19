import { fireEvent, render, screen } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import AnnouncerTab from "./components/AnnouncerTab.svelte";

const mockState = vi.hoisted(() => ({
  store: {
    announcerEnabled: false,
    announcerBusy: false,
    importBusy: false,
    importMessage: null as string | null,
    importError: null as string | null,
  },
  setAnnouncerEnabled: vi.fn(),
  importParticipantsText: vi.fn(),
  importChipsText: vi.fn(),
}));

vi.mock("$lib/store.svelte", () => ({
  store: mockState.store,
  setAnnouncerEnabled: mockState.setAnnouncerEnabled,
  importParticipantsText: mockState.importParticipantsText,
  importChipsText: mockState.importChipsText,
}));

describe("AnnouncerTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockState.store.announcerEnabled = false;
    mockState.store.announcerBusy = false;
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
});
