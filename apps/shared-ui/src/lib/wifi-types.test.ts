import { describe, it, expect } from "vitest";
import {
  isNetworkSecured,
  signalBars,
  wifiStateBadge,
  isLikelyNetworkDropError,
  extractWifiErrorMessage,
  type WifiStatus,
} from "./wifi-types";

describe("wifi-types helpers", () => {
  describe("isNetworkSecured", () => {
    it("returns false for open or empty security strings", () => {
      expect(isNetworkSecured("")).toBe(false);
      expect(isNetworkSecured(null)).toBe(false);
      expect(isNetworkSecured(undefined)).toBe(false);
      expect(isNetworkSecured("Open")).toBe(false);
      expect(isNetworkSecured("open")).toBe(false);
      expect(isNetworkSecured("--")).toBe(false);
      expect(isNetworkSecured("none")).toBe(false);
    });

    it("returns true for WPA2/WPA3 security strings", () => {
      expect(isNetworkSecured("WPA2")).toBe(true);
      expect(isNetworkSecured("WPA2 WPA3")).toBe(true);
      expect(isNetworkSecured("WPA3")).toBe(true);
    });
  });

  describe("signalBars", () => {
    it("maps signal percentages to 0..4 bars", () => {
      expect(signalBars(null)).toBe(0);
      expect(signalBars(undefined)).toBe(0);
      expect(signalBars(0)).toBe(0);
      expect(signalBars(1)).toBe(1);
      expect(signalBars(25)).toBe(1);
      expect(signalBars(26)).toBe(2);
      expect(signalBars(50)).toBe(2);
      expect(signalBars(51)).toBe(3);
      expect(signalBars(75)).toBe(3);
      expect(signalBars(76)).toBe(4);
      expect(signalBars(100)).toBe(4);
    });
  });

  describe("wifiStateBadge", () => {
    const baseStatus: WifiStatus = {
      available: true,
      enabled: true,
      state: "connected",
      saved_networks: [],
    };

    it("returns unavailable/err when not available", () => {
      expect(wifiStateBadge({ ...baseStatus, available: false })).toEqual({
        label: "unavailable",
        state: "err",
      });
    });

    it("returns disabled/err when radio is disabled", () => {
      expect(wifiStateBadge({ ...baseStatus, enabled: false })).toEqual({
        label: "disabled",
        state: "err",
      });
    });

    it("returns connected/ok when connected", () => {
      expect(wifiStateBadge(baseStatus)).toEqual({
        label: "connected",
        state: "ok",
      });
    });

    it("returns warn for connecting or disconnected", () => {
      expect(wifiStateBadge({ ...baseStatus, state: "connecting" })).toEqual({
        label: "connecting",
        state: "warn",
      });
      expect(wifiStateBadge({ ...baseStatus, state: "disconnected" })).toEqual({
        label: "disconnected",
        state: "warn",
      });
    });
  });

  describe("isLikelyNetworkDropError", () => {
    it("detects fetch and timeout errors", () => {
      expect(isLikelyNetworkDropError(new TypeError("Failed to fetch"))).toBe(
        true,
      );
      expect(
        isLikelyNetworkDropError(
          new Error("forwarder wifi request timed out after 25s"),
        ),
      ).toBe(true);
      expect(
        isLikelyNetworkDropError(new Error("Invalid password for SSID")),
      ).toBe(false);
    });
  });

  describe("extractWifiErrorMessage", () => {
    it("unwraps JSON error payloads from ApiError messages", () => {
      const err = new Error(
        'API POST /api/v1/wifi/connect -> 500: {"ok":false,"error":"Failed to connect to \'RaceWiFi\': bad password"}',
      );
      expect(extractWifiErrorMessage(err)).toBe(
        "Failed to connect to 'RaceWiFi': bad password",
      );
    });

    it("returns plain error messages unchanged", () => {
      expect(extractWifiErrorMessage(new Error("wifi control disabled"))).toBe(
        "wifi control disabled",
      );
    });
  });
});
