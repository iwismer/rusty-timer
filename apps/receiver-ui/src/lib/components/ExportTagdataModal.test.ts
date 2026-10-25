import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  getExportEpochs: vi.fn(),
  exportTagdata: vi.fn(),
}));

const dialogMocks = vi.hoisted(() => ({
  save: vi.fn(),
}));

vi.mock("$lib/api", () => apiMocks);
vi.mock("@tauri-apps/plugin-dialog", () => dialogMocks);

import ExportTagdataModal from "./ExportTagdataModal.svelte";
import { store, streamIdentity } from "$lib/store.svelte";

describe("ExportTagdataModal", () => {
  const stream1 = {
    forwarder_endpoint_id: "fwd1",
    stream_id: "s1",
    reader_ip: "192.168.1.10",
    display_alias: "Finish Line",
    subscribed: true,
    local_port: 10001,
  };
  const stream2 = {
    forwarder_endpoint_id: "fwd1",
    stream_id: "s2",
    reader_ip: "192.168.1.11",
    display_alias: "Start Line",
    subscribed: true,
    local_port: 10002,
  };

  const key1 = streamIdentity(stream1);
  const key2 = streamIdentity(stream2);

  beforeEach(() => {
    vi.clearAllMocks();
    store.streams = {
      streams: [stream1, stream2],
      degraded: false,
      upstream_error: null,
    };
    dialogMocks.save.mockResolvedValue("/path/to/TAGDATA.TXT");
    apiMocks.getExportEpochs.mockResolvedValue({
      epochs: [2, 1],
      details: [
        { epoch: 2, name: "Race 2", read_count: 120 },
        { epoch: 1, name: "Race 1", read_count: 80 },
      ],
      streams: [
        {
          stream_id: key1,
          epochs: [
            { epoch: 2, name: "Race 2", read_count: 120 },
            { epoch: 1, name: "Race 1", read_count: 80 },
          ],
        },
        {
          stream_id: key2,
          epochs: [{ epoch: 1, name: "5K Start", read_count: 45 }],
        },
      ],
    });
    apiMocks.exportTagdata.mockResolvedValue({
      read_count: 245,
      filename: "TAGDATA.TXT",
      written_paths: ["/path/to/TAGDATA.TXT"],
      content: null,
      zip_base64: null,
    });
  });

  it("renders per-stream epoch selection with epoch name and read count for all streams", async () => {
    render(ExportTagdataModal, {
      open: true,
      initialStreamId: null,
      onclose: vi.fn(),
    });

    await waitFor(() => {
      expect(
        screen.getByTestId(`receiver-export-epoch-stream-${key1}`),
      ).toBeInTheDocument();
      expect(
        screen.getByTestId(`receiver-export-epoch-stream-${key2}`),
      ).toBeInTheDocument();
    });

    const select1 = screen.getByTestId(
      `receiver-export-epoch-stream-${key1}`,
    ) as HTMLSelectElement;
    const options1 = Array.from(select1.options).map((o) => o.text);
    expect(options1).toContain("Epoch 2 (Race 2) — 120 reads");
    expect(options1).toContain("Epoch 1 (Race 1) — 80 reads");

    const select2 = screen.getByTestId(
      `receiver-export-epoch-stream-${key2}`,
    ) as HTMLSelectElement;
    const options2 = Array.from(select2.options).map((o) => o.text);
    expect(options2).toContain("Epoch 1 (5K Start) — 45 reads");
  });

  it("allows selecting different epochs per stream and exports with stream_epochs payload", async () => {
    render(ExportTagdataModal, {
      open: true,
      initialStreamId: null,
      onclose: vi.fn(),
    });

    await waitFor(() => {
      expect(
        screen.getByTestId(`receiver-export-epoch-stream-${key1}`),
      ).toBeInTheDocument();
    });

    const select1 = screen.getByTestId(
      `receiver-export-epoch-stream-${key1}`,
    ) as HTMLSelectElement;
    await fireEvent.change(select1, { target: { value: "2" } });

    const downloadBtn = screen.getByTestId("export-download-btn");
    await fireEvent.click(downloadBtn);

    await waitFor(() => {
      expect(apiMocks.exportTagdata).toHaveBeenCalledWith({
        mode: "merged",
        stream_epochs: {
          [key1]: 2,
          [key2]: null,
        },
        destination_dir: "/path/to/TAGDATA.TXT",
      });
    });
  });

  it("quick action buttons set all streams to All Reads or Latest Epoch", async () => {
    render(ExportTagdataModal, {
      open: true,
      initialStreamId: null,
      onclose: vi.fn(),
    });

    await waitFor(() => {
      expect(
        screen.getByTestId(`receiver-export-epoch-stream-${key1}`),
      ).toBeInTheDocument();
    });

    const latestBtn = screen.getByRole("button", { name: "Latest Epoch" });
    await fireEvent.click(latestBtn);

    const select1 = screen.getByTestId(
      `receiver-export-epoch-stream-${key1}`,
    ) as HTMLSelectElement;
    const select2 = screen.getByTestId(
      `receiver-export-epoch-stream-${key2}`,
    ) as HTMLSelectElement;

    expect(select1.value).toBe("2");
    expect(select2.value).toBe("1");

    const allReadsBtn = screen.getByRole("button", { name: "All Reads" });
    await fireEvent.click(allReadsBtn);

    expect(select1.value).toBe("all");
    expect(select2.value).toBe("all");
  });

  it("renders single stream export with epoch name and read count when target is single stream", async () => {
    render(ExportTagdataModal, {
      open: true,
      initialStreamId: key1,
      onclose: vi.fn(),
    });

    await waitFor(() => {
      const select = document.getElementById(
        "receiver-export-epoch-select",
      ) as HTMLSelectElement;
      expect(select).toBeInTheDocument();
    });

    const select = document.getElementById(
      "receiver-export-epoch-select",
    ) as HTMLSelectElement;
    const options = Array.from(select.options).map((o) => o.text);
    expect(options).toContain("Epoch 2 (Race 2) — 120 reads");
    expect(options).toContain("Epoch 1 (Race 1) — 80 reads");

    await fireEvent.change(select, { target: { value: "2" } });

    const downloadBtn = screen.getByTestId("export-download-btn");
    await fireEvent.click(downloadBtn);

    await waitFor(() => {
      expect(apiMocks.exportTagdata).toHaveBeenCalledWith({
        stream_id: key1,
        epoch: 2,
        mode: "merged",
        destination_dir: "/path/to/TAGDATA.TXT",
      });
    });
  });
});
