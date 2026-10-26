import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import RaceDayStartupModal from "./RaceDayStartupModal.svelte";
import {
  store,
  setStreamRole,
  openRaceDayPrompt,
  openRaceDayWizard,
} from "$lib/store.svelte";

const apiMocks = vi.hoisted(() => ({
  getDbfConfig: vi.fn().mockResolvedValue({
    enabled: true,
    flush_interval_ms: 1000,
  }),
  putDbfConfig: vi.fn().mockResolvedValue({}),
  getRdImportConfig: vi.fn().mockResolvedValue({
    enabled: true,
    dir: "C:\\Winrace\\Files",
    interval_secs: 15,
  }),
  putRdImportConfig: vi.fn().mockResolvedValue({}),
  clearDbf: vi.fn().mockResolvedValue({}),
  resetAllStreamsData: vi.fn().mockResolvedValue({ reset_count: 2 }),
  readerAdvanceEpoch: vi.fn().mockResolvedValue({
    success: true,
    message: "Epoch advanced",
    new_epoch: 2,
  }),
  readerSyncClock: vi.fn().mockResolvedValue({
    success: true,
    message: "Clock synced",
  }),
  updateSubscriptionEventType: vi.fn().mockResolvedValue({}),
  setStreamAnnouncerPublish: vi.fn().mockResolvedValue({}),
  setAnnouncerEnabled: vi.fn().mockResolvedValue({}),
  setAnnouncerMaxListSize: vi.fn().mockResolvedValue({}),
  resetAnnouncer: vi.fn().mockResolvedValue({}),
  getDataStats: vi.fn().mockResolvedValue({
    participants: 524,
    chips: 524,
    matched_participants: 524,
    participants_without_chips: 0,
    resolvable_chips: 524,
  }),
  getStreams: vi.fn().mockImplementation(async () => ({
    streams: store.streams?.streams ?? [],
    degraded: false,
    upstream_error: null,
  })),
  getStreamEpochs: vi.fn().mockResolvedValue({
    options: [],
    supports_earliest_epoch: false,
  }),
}));

vi.mock("$lib/api", () => ({
  ...apiMocks,
}));

describe("RaceDayStartupModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    sessionStorage.clear();
    localStorage.clear();

    store.raceDayModalOpen = false;
    store.raceDayModalMode = "prompt";
    store.streamRoles = {};
    store.streamCourseOrder = [];
    store.cutoverEpochName = "Finish Reads";

    store.dataStats = {
      participants: 524,
      chips: 524,
      matched_participants: 524,
      participants_without_chips: 0,
      resolvable_chips: 524,
    };

    store.connections = {
      server: {
        configured: true,
        endpoint_id: "srv-1",
        reachable: true,
        approval_state: "active",
        waiting_for_approval: false,
        message: null,
      },
      forwarders: [
        {
          endpoint_id: "fwd-1",
          display_name: "box-01",
          state: "subscribed",
          pending: false,
          subscribed_count: 2,
          available_count: 2,
          last_seen_secs: 0,
          ups: null,
          restart_needed: false,
          remote_config_available: false,
          readers: [
            {
              stream_id: "reader-100",
              connected: true,
              state: "connected",
              last_read_unix_ms: null,
              hardware_reader_id: null,
              firmware_version: null,
              model: null,
              reader_info: {
                clock: {
                  drift_ms: 1,
                  reader_clock: "2026-10-25T12:00:00Z",
                },
              },
            },
            {
              stream_id: "reader-101",
              connected: true,
              state: "connected",
              last_read_unix_ms: null,
              hardware_reader_id: null,
              firmware_version: null,
              model: null,
              reader_info: {
                clock: {
                  drift_ms: -2,
                  reader_clock: "2026-10-25T12:00:00Z",
                },
              },
            },
          ],
        },
      ],
    };

    store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "reader-100",
          forwarder_id: "box-01",
          reader_ip: "192.168.1.100",
          subscribed: true,
          local_port: 10000,
          event_type: "start",
          stream_epoch: 1,
          current_epoch_name: "Mass Start",
          reads_epoch: 10,
          reads_total: 100,
        },
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "reader-101",
          forwarder_id: "box-01",
          reader_ip: "192.168.1.101",
          subscribed: true,
          local_port: 10001,
          event_type: "finish",
          stream_epoch: 1,
          current_epoch_name: "Finish Reads",
          reads_epoch: 20,
          reads_total: 200,
        },
      ],
      degraded: false,
      upstream_error: null,
    };
  });

  it("prompts user on initial launch and allows resuming previous session", async () => {
    openRaceDayPrompt();
    render(RaceDayStartupModal);

    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByText("Race Day Setup")).toBeInTheDocument();
    expect(
      screen.getByText(/Starting a new race day will walk you through/i),
    ).toBeInTheDocument();

    const resumeBtn = screen.getByTestId("race-day-resume-btn");
    await fireEvent.click(resumeBtn);

    expect(store.raceDayModalOpen).toBe(false);
    expect(sessionStorage.getItem("rusty_timer_race_day_prompted")).toBe(
      "true",
    );
  });

  it("entering wizard walks through steps and supports navigation", async () => {
    openRaceDayPrompt();
    render(RaceDayStartupModal);

    const newBtn = screen.getByTestId("race-day-new-btn");
    await fireEvent.click(newBtn);

    // Enters step 1
    expect(store.raceDayModalMode).toBe("wizard");
    expect(screen.getByText("Step 1 of 7")).toBeInTheDocument();
    expect(screen.getByText("Select Forwarders & Readers")).toBeInTheDocument();
    expect(screen.getByText("box-01")).toBeInTheDocument();

    // Step 1 -> Step 2
    const nextBtn = screen.getByTestId("wizard-next-btn");
    await fireEvent.click(nextBtn);

    await waitFor(() => {
      expect(screen.getByText("Step 2 of 7")).toBeInTheDocument();
    });
    expect(
      screen.getByText("Assign Reader Roles & Course Order"),
    ).toBeInTheDocument();
    expect(screen.getByText("#1")).toBeInTheDocument();
    expect(screen.getByText("#2")).toBeInTheDocument();

    // Back to Step 1
    const prevBtn = screen.getByTestId("wizard-prev-btn");
    await fireEvent.click(prevBtn);
    await waitFor(() => {
      expect(screen.getByText("Step 1 of 7")).toBeInTheDocument();
    });
  });

  it("Step 3 saves configs and auto-skips Step 4 when participants exist", async () => {
    openRaceDayWizard();
    render(RaceDayStartupModal);

    const nextBtn = screen.getByTestId("wizard-next-btn");
    // Step 1 -> 2
    await fireEvent.click(nextBtn);
    await waitFor(() => {
      expect(screen.getByText("Step 2 of 7")).toBeInTheDocument();
    });

    // Step 2 -> 3
    await fireEvent.click(nextBtn);
    await waitFor(() => {
      expect(screen.getByText("Step 3 of 7")).toBeInTheDocument();
    });

    expect(screen.getByText("524 participants")).toBeInTheDocument();

    // Step 3 -> Next should save and skip Step 4 directly to Step 5 (Announcer Screen Setup)
    await fireEvent.click(nextBtn);

    await waitFor(() => {
      expect(apiMocks.putDbfConfig).toHaveBeenCalled();
      expect(apiMocks.putRdImportConfig).toHaveBeenCalled();
      expect(screen.getByText("Step 5 of 7")).toBeInTheDocument();
      expect(screen.getByText("Announcer Screen Setup")).toBeInTheDocument();
    });

    // Clicking Prev from Step 5 should skip back to Step 3
    const prevBtn = screen.getByTestId("wizard-prev-btn");
    await fireEvent.click(prevBtn);
    await waitFor(() => {
      expect(screen.getByText("Step 3 of 7")).toBeInTheDocument();
    });
  });

  it("Step 5 configures announcer screen settings", async () => {
    openRaceDayWizard();
    render(RaceDayStartupModal);

    const nextBtn = screen.getByTestId("wizard-next-btn");
    await fireEvent.click(nextBtn); // to 2
    await waitFor(() =>
      expect(screen.getByText("Step 2 of 7")).toBeInTheDocument(),
    );

    await fireEvent.click(nextBtn); // to 3
    await waitFor(() =>
      expect(screen.getByText("Step 3 of 7")).toBeInTheDocument(),
    );

    await fireEvent.click(nextBtn); // to 5 (skipping 4)
    await waitFor(() =>
      expect(screen.getByText("Step 5 of 7")).toBeInTheDocument(),
    );

    const toggle = screen.getByTestId("wizard-announcer-enable-toggle");
    expect(toggle).toBeInTheDocument();

    const maxList = screen.getByTestId("wizard-announcer-max-list");
    expect(maxList).toBeInTheDocument();

    await fireEvent.click(toggle);
    await fireEvent.input(maxList, { target: { value: "30" } });

    await fireEvent.click(nextBtn); // to 6
    await waitFor(() => {
      expect(apiMocks.setAnnouncerEnabled).toHaveBeenCalledWith(true);
      expect(apiMocks.setAnnouncerMaxListSize).toHaveBeenCalledWith(30);
      expect(screen.getByText("Step 6 of 7")).toBeInTheDocument();
    });
  });

  it("Step 6 displays clock drift chips and supports synchronizing all reader clocks", async () => {
    openRaceDayWizard();
    render(RaceDayStartupModal);

    const nextBtn = screen.getByTestId("wizard-next-btn");
    await fireEvent.click(nextBtn); // to 2
    await waitFor(() =>
      expect(screen.getByText("Step 2 of 7")).toBeInTheDocument(),
    );

    await fireEvent.click(nextBtn); // to 3
    await waitFor(() =>
      expect(screen.getByText("Step 3 of 7")).toBeInTheDocument(),
    );

    await fireEvent.click(nextBtn); // to 5 (skipping 4)
    await waitFor(() =>
      expect(screen.getByText("Step 5 of 7")).toBeInTheDocument(),
    );

    await fireEvent.click(nextBtn); // to 6
    await waitFor(() =>
      expect(screen.getByText("Step 6 of 7")).toBeInTheDocument(),
    );

    expect(screen.getByText("+1ms")).toBeInTheDocument();
    expect(screen.getByText("-2ms")).toBeInTheDocument();

    const syncBtn = screen.getByTestId("sync-all-clocks-btn");
    await fireEvent.click(syncBtn);

    await waitFor(() => {
      expect(apiMocks.readerSyncClock).toHaveBeenCalledTimes(2);
    });
  });

  it("Step 7 renders epoch groups according to roles and completes setup", async () => {
    // Configure reader-100 as start_finish and reader-101 as finish
    setStreamRole(store.streams!.streams[0], "start_finish");
    setStreamRole(store.streams!.streams[1], "finish");

    openRaceDayWizard();
    render(RaceDayStartupModal);

    const nextBtn = screen.getByTestId("wizard-next-btn");
    await fireEvent.click(nextBtn); // to 2
    await waitFor(() =>
      expect(screen.getByText("Step 2 of 7")).toBeInTheDocument(),
    );

    // Verify "Also publish reads to Announcer screen" toggle is rendered for start_finish
    const announcerCheckbox = screen.getByTestId(
      "role-announcer-toggle-fwd-1/reader-100",
    );
    expect(announcerCheckbox).toBeInTheDocument();
    await fireEvent.click(announcerCheckbox);
    expect(apiMocks.setStreamAnnouncerPublish).toHaveBeenCalledWith(
      "fwd-1",
      "reader-100",
      true,
    );

    await fireEvent.click(nextBtn); // to 3
    await waitFor(() =>
      expect(screen.getByText("Step 3 of 7")).toBeInTheDocument(),
    );

    await fireEvent.click(nextBtn); // to 5
    await waitFor(() =>
      expect(screen.getByText("Step 5 of 7")).toBeInTheDocument(),
    );

    await fireEvent.click(nextBtn); // to 6
    await waitFor(() =>
      expect(screen.getByText("Step 6 of 7")).toBeInTheDocument(),
    );

    await fireEvent.click(nextBtn); // to 7
    await waitFor(() =>
      expect(screen.getByText("Step 7 of 7")).toBeInTheDocument(),
    );

    expect(screen.getByText("Epoch Names & Reset")).toBeInTheDocument();

    // Start / Finish group exists
    const sfStartInput = screen.getByTestId("sf-start-epoch-input");
    const sfFinishInput = screen.getByTestId("sf-finish-epoch-input");
    expect(sfStartInput).toBeInTheDocument();
    expect(sfFinishInput).toBeInTheDocument();

    // Finish group exists
    const finishInput = screen.getByTestId("finish-epoch-input");
    expect(finishInput).toBeInTheDocument();

    // Announcer group does not exist (no announcer readers)
    expect(screen.queryByTestId("announcer-epoch-input")).toBeNull();

    // Customize epoch names
    await fireEvent.input(sfStartInput, {
      target: { value: "10K Mass Start" },
    });
    await fireEvent.input(sfFinishInput, { target: { value: "10K Finish" } });
    await fireEvent.input(finishInput, {
      target: { value: "10K Backup Finish" },
    });

    // Complete race day setup
    const finishBtn = screen.getByRole("button", {
      name: /Finish & Start Race Day/i,
    });
    await fireEvent.click(finishBtn);

    await waitFor(() => {
      // Advance epoch called for both readers with their respective group epoch names
      expect(apiMocks.readerAdvanceEpoch).toHaveBeenCalledWith(
        "fwd-1",
        "reader-100",
        "10K Mass Start",
      );
      expect(apiMocks.readerAdvanceEpoch).toHaveBeenCalledWith(
        "fwd-1",
        "reader-101",
        "10K Backup Finish",
      );
      // Cutover finish epoch stored in cutoverEpochName
      expect(store.cutoverEpochName).toBe("10K Finish");
      // Local test reads and DBF cleared
      expect(apiMocks.resetAllStreamsData).toHaveBeenCalled();
      expect(apiMocks.clearDbf).toHaveBeenCalled();
      // Announcer feed reset requested by default
      expect(apiMocks.resetAnnouncer).toHaveBeenCalled();
      // Modal closed
      expect(store.raceDayModalOpen).toBe(false);
    });
  });

  it("supports resetting online announcer feed immediately in Step 5 and toggling auto-reset", async () => {
    openRaceDayWizard();
    render(RaceDayStartupModal);

    const nextBtn = screen.getByTestId("wizard-next-btn");
    await fireEvent.click(nextBtn); // to 2
    await fireEvent.click(nextBtn); // to 3
    await fireEvent.click(nextBtn); // to 5 (skips 4 due to participants)

    await waitFor(() => {
      expect(screen.getByText("Step 5 of 7")).toBeInTheDocument();
    });

    const resetToggle = screen.getByTestId("wizard-announcer-reset-toggle");
    expect(resetToggle).toBeInTheDocument();
    expect(resetToggle).toBeChecked();

    const resetNowBtn = screen.getByTestId("wizard-announcer-reset-now-btn");
    expect(resetNowBtn).toBeInTheDocument();

    await fireEvent.click(resetNowBtn);
    expect(apiMocks.resetAnnouncer).toHaveBeenCalledTimes(1);

    await waitFor(() => {
      expect(
        screen.getByTestId("wizard-announcer-reset-success"),
      ).toHaveTextContent(/Announcer feed reset for the next race/i);
    });

    // Advance to Step 6 and then Step 7
    await fireEvent.click(nextBtn); // to 6
    await waitFor(() => {
      expect(screen.getByText("Step 6 of 7")).toBeInTheDocument();
    });

    await fireEvent.click(nextBtn); // to 7
    await waitFor(() => {
      expect(screen.getByText("Step 7 of 7")).toBeInTheDocument();
    });

    expect(
      screen.getByTestId("wizard-summary-announcer-reset"),
    ).toHaveTextContent(/Online Announcer feed will be reset/i);
  });

  it("does not clear DBF file when write reads to IPICO.DBF is unchecked", async () => {
    // DBF writing is disabled
    apiMocks.getDbfConfig.mockResolvedValueOnce({
      enabled: false,
      flush_interval_ms: 1000,
    });

    openRaceDayWizard();
    render(RaceDayStartupModal);

    const nextBtn = screen.getByTestId("wizard-next-btn");
    await fireEvent.click(nextBtn); // to 2
    await fireEvent.click(nextBtn); // to 3

    await waitFor(() => {
      expect(screen.getByText("Step 3 of 7")).toBeInTheDocument();
    });

    // Advance through the remaining steps to Step 7
    await fireEvent.click(nextBtn); // to 5 (skips 4 due to participants)
    await waitFor(() => {
      expect(screen.getByText("Step 5 of 7")).toBeInTheDocument();
    });

    await fireEvent.click(nextBtn); // to 6
    await waitFor(() => {
      expect(screen.getByText("Step 6 of 7")).toBeInTheDocument();
    });

    await fireEvent.click(nextBtn); // to 7
    await waitFor(() => {
      expect(screen.getByText("Step 7 of 7")).toBeInTheDocument();
    });

    // Label should only say "Clear local test reads" (no DBF mention)
    const clearReadsLabel = screen.getByTestId("wizard-clear-reads-label");
    expect(clearReadsLabel).toHaveTextContent("Clear local test reads");
    expect(clearReadsLabel).not.toHaveTextContent("reset DBF");

    // Complete setup
    const finishBtn = screen.getByRole("button", {
      name: /Finish & Start Race Day/i,
    });
    await fireEvent.click(finishBtn);

    await waitFor(() => {
      // Local test reads ARE reset
      expect(apiMocks.resetAllStreamsData).toHaveBeenCalled();
      // DBF clear is NOT called
      expect(apiMocks.clearDbf).not.toHaveBeenCalled();
      // Modal closed
      expect(store.raceDayModalOpen).toBe(false);
    });
  });
});
