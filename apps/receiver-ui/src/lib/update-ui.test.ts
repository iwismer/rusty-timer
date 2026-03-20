import { fireEvent, render, screen } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import StatusBar from "./components/StatusBar.svelte";
import UpdateModal from "./components/UpdateModal.svelte";
import layoutSource from "../routes/+layout.svelte?raw";

const mockState = vi.hoisted(() => {
  const store = {
    streams: { streams: [] },
    status: { receiver_id: "recv-test" },
    receiverVersion: "",
    appVersion: "0.8.0",
    updateVersion: "0.9.0",
    updateStatus: "available",
    updateModalOpen: false,
    updateState: {
      status: "available",
      currentVersion: "0.8.0",
      version: "0.9.0",
      notes: "Fixes receiver update UX",
      busy: false,
      error: null,
    },
  };

  return {
    store,
    openUpdateModal: vi.fn(() => {
      store.updateModalOpen = true;
    }),
    closeUpdateModal: vi.fn(() => {
      store.updateModalOpen = false;
    }),
    confirmUpdateInstall: vi.fn(),
  };
});

vi.mock("$lib/store.svelte", () => ({
  store: mockState.store,
  openUpdateModal: mockState.openUpdateModal,
  closeUpdateModal: mockState.closeUpdateModal,
  confirmUpdateInstall: mockState.confirmUpdateInstall,
}));

describe("receiver update UI", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockState.store.updateModalOpen = false;
    mockState.store.updateState = {
      status: "available",
      currentVersion: "0.8.0",
      version: "0.9.0",
      notes: "Fixes receiver update UX",
      busy: false,
      error: null,
    };
  });

  it("removes the shared top-of-page update banner from the layout", () => {
    expect(layoutSource).not.toContain("UpdateBanner");
  });

  it("shows the desktop app version and update indicator in the status bar", async () => {
    render(StatusBar);

    expect(screen.getByText("v0.8.0")).toBeInTheDocument();

    const button = screen.getByTestId("update-indicator-btn");
    await fireEvent.click(button);

    expect(mockState.openUpdateModal).toHaveBeenCalledTimes(1);
  });

  it("renders update release notes in the modal when present", () => {
    mockState.store.updateModalOpen = true;

    render(UpdateModal);

    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByText("Update available")).toBeInTheDocument();
    expect(screen.getByText("Fixes receiver update UX")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Download and install" }),
    ).toBeInTheDocument();
  });
});
