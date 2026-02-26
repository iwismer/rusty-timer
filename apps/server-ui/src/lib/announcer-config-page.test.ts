import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";

vi.mock("$lib/api", async () => {
  const actual = await vi.importActual<typeof import("$lib/api")>("$lib/api");
  return {
    ...actual,
    getStreams: vi.fn(),
    getAnnouncerConfig: vi.fn(),
    updateAnnouncerConfig: vi.fn(),
    resetAnnouncer: vi.fn(),
  };
});

import * as api from "$lib/api";
import AnnouncerConfigPage from "../routes/announcer-config/+page.svelte";

describe("announcer config page", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.getStreams).mockResolvedValue({
      streams: [
        {
          stream_id: "stream-1",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          display_alias: "Main Reader",
          forwarder_display_name: null,
          online: true,
          stream_epoch: 1,
          created_at: "2026-02-26T10:00:00Z",
        },
      ],
    });
    vi.mocked(api.getAnnouncerConfig).mockResolvedValue({
      enabled: false,
      enabled_until: null,
      selected_stream_ids: [],
      max_list_size: 25,
      updated_at: "2026-02-26T10:00:00Z",
      public_enabled: false,
    });
    vi.mocked(api.updateAnnouncerConfig).mockResolvedValue({
      enabled: true,
      enabled_until: "2026-02-27T10:00:00Z",
      selected_stream_ids: ["stream-1"],
      max_list_size: 25,
      updated_at: "2026-02-26T10:05:00Z",
      public_enabled: true,
    });
    vi.mocked(api.resetAnnouncer).mockResolvedValue();
  });

  it("disables save when announcer is enabled with no selected streams", async () => {
    render(AnnouncerConfigPage);

    const saveButton = await screen.findByTestId("announcer-save-btn");
    const enableCheckbox = screen.getByLabelText("Enable announcer");

    expect(saveButton).toBeEnabled();
    await fireEvent.click(enableCheckbox);
    expect(saveButton).toBeDisabled();
  });

  it("shows a link to open the public announcer page", async () => {
    render(AnnouncerConfigPage);

    const link = await screen.findByRole("link", {
      name: "Open announcer page",
    });
    expect(link).toHaveAttribute("href", "/announcer");
  });
});
