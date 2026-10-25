export interface SavedWifiNetwork {
  ssid: string;
  uuid: string;
  autoconnect: boolean;
}

export interface WifiStatus {
  available: boolean;
  enabled: boolean;
  interface?: string | null;
  state: string;
  ssid?: string | null;
  ip_address?: string | null;
  gateway?: string | null;
  signal?: number | null;
  security?: string | null;
  band?: string | null;
  saved_networks: SavedWifiNetwork[];
}

export interface WifiNetwork {
  ssid: string;
  in_use: boolean;
  signal: number;
  security: string;
  bands: string[];
  saved: boolean;
}

export interface WifiScanResult {
  status: WifiStatus;
  networks: WifiNetwork[];
}

export interface WifiActionResult {
  ok: boolean;
  message: string;
  status?: WifiStatus | null;
}

export interface WifiApi {
  getStatus(): Promise<WifiStatus>;
  scan(rescan?: boolean): Promise<WifiScanResult>;
  connect(params: {
    ssid: string;
    password?: string | null;
    hidden?: boolean;
  }): Promise<WifiActionResult>;
  disconnect(): Promise<WifiActionResult>;
  forget(ssid: string): Promise<WifiActionResult>;
}

/**
 * Returns true if the network requires a password (not Open / -- / none).
 */
export function isNetworkSecured(security: string | null | undefined): boolean {
  if (!security) return false;
  const normalized = security.trim().toLowerCase();
  return (
    normalized.length > 0 &&
    normalized !== "open" &&
    normalized !== "--" &&
    normalized !== "none"
  );
}

/**
 * Maps a 0..100 signal percentage to 0..4 signal bars.
 */
export function signalBars(
  signal: number | null | undefined,
): 0 | 1 | 2 | 3 | 4 {
  if (signal == null || Number.isNaN(signal) || signal <= 0) return 0;
  if (signal <= 25) return 1;
  if (signal <= 50) return 2;
  if (signal <= 75) return 3;
  return 4;
}

/**
 * Maps WifiStatus to a badge label and state ("ok" | "warn" | "err").
 */
export function wifiStateBadge(status: WifiStatus): {
  label: string;
  state: "ok" | "warn" | "err";
} {
  if (!status.available) {
    return { label: "unavailable", state: "err" };
  }
  if (!status.enabled) {
    return { label: "disabled", state: "err" };
  }
  if (status.state === "connected") {
    return { label: "connected", state: "ok" };
  }
  if (status.state === "connecting") {
    return { label: "connecting", state: "warn" };
  }
  return { label: status.state || "disconnected", state: "warn" };
}

/**
 * Detects whether an error thrown during connect/disconnect is likely caused
 * by the forwarder switching Wi-Fi networks and dropping the current HTTP/P2P link.
 */
export function isLikelyNetworkDropError(err: unknown): boolean {
  const message =
    err instanceof Error
      ? err.message
      : typeof err === "string"
        ? err
        : String(err ?? "");
  const lower = message.toLowerCase();
  return (
    lower.includes("failed to fetch") ||
    lower.includes("networkerror") ||
    lower.includes("load failed") ||
    lower.includes("network request failed") ||
    lower.includes("timed out") ||
    lower.includes("connection closed") ||
    lower.includes("not connected")
  );
}

/**
 * Extracts a clean user-facing error message from an Error / ApiError or string,
 * unwrapping JSON error payloads like `API POST /api/v1/wifi/connect -> 500: {"ok":false,"error":"..."}`.
 */
export function extractWifiErrorMessage(err: unknown): string {
  const raw =
    err instanceof Error
      ? err.message
      : typeof err === "string"
        ? err
        : String(err ?? "Unknown Wi-Fi error");
  const jsonStart = raw.indexOf("{");
  if (jsonStart >= 0) {
    try {
      const parsed = JSON.parse(raw.slice(jsonStart)) as Record<
        string,
        unknown
      >;
      if (typeof parsed.error === "string" && parsed.error.trim().length > 0) {
        return parsed.error.trim();
      }
      if (
        typeof parsed.message === "string" &&
        parsed.message.trim().length > 0
      ) {
        return parsed.message.trim();
      }
    } catch {
      // Fall through to raw string
    }
  }
  return raw;
}
