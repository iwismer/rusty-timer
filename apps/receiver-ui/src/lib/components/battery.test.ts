import { render, screen } from "@testing-library/svelte";
import { describe, expect, it } from "vitest";

import { BatteryIndicator, LowBatteryBanner } from "@rusty-timer/shared-ui";

describe("BatteryIndicator", () => {
  it("renders dash when not configured", () => {
    render(BatteryIndicator, {
      props: { percent: null, configured: false },
    });

    expect(screen.getByText("\u2014")).toBeInTheDocument();
    expect(screen.getByTitle("No UPS configured")).toBeInTheDocument();
  });

  it("renders percent text when available", () => {
    render(BatteryIndicator, {
      props: { percent: 75, configured: true, available: true },
    });

    expect(screen.getByText("75%")).toBeInTheDocument();
  });

  it("shows UPS unavailable title when available is false", () => {
    render(BatteryIndicator, {
      props: { percent: 50, configured: true, available: false },
    });

    expect(screen.getByTitle("UPS unavailable")).toBeInTheDocument();
    expect(screen.getByText("UPS unavailable")).toBeInTheDocument();
  });
});

describe("LowBatteryBanner", () => {
  it("renders nothing when no low-battery forwarders", () => {
    const { container } = render(LowBatteryBanner, {
      props: { forwarders: [] },
    });

    expect(container.querySelector(".bg-red-600")).toBeNull();
    expect(screen.queryByText("Low battery:")).not.toBeInTheDocument();
  });

  it("shows warning for one low-battery forwarder", () => {
    render(LowBatteryBanner, {
      props: {
        forwarders: [{ name: "Start Line", percent: 15 }],
      },
    });

    expect(screen.getByText(/Low battery:/)).toBeInTheDocument();
    expect(screen.getByText(/Start Line at 15%/)).toBeInTheDocument();
  });

  it("shows multiple low-battery forwarders together", () => {
    render(LowBatteryBanner, {
      props: {
        forwarders: [
          { name: "Start Line", percent: 10 },
          { name: "Finish Line", percent: 5 },
        ],
      },
    });

    expect(screen.getByText(/Low battery:/)).toBeInTheDocument();
    expect(screen.getByText(/Start Line at 10%/)).toBeInTheDocument();
    expect(screen.getByText(/Finish Line at 5%/)).toBeInTheDocument();
  });
});
