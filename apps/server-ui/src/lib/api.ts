import { apiFetch } from "@rusty-timer/shared-ui/lib/api-helpers";

export type DeviceKind = "forwarder" | "receiver";
export type ApprovalState = "pending" | "active";

export interface DeviceRecord {
  endpoint_id: string;
  device_kind: DeviceKind;
  display_name: string | null;
  approval_state: ApprovalState;
}

export interface ForwarderStreamRecord {
  stream_id: string;
  endpoint_id: string;
  epoch: number;
  next_seq: number;
}

export interface AnnouncerRow {
  stream_id: string;
  seq: number;
  chip_id: string;
  bib: number | null;
  display_name: string;
  reader_timestamp: string | null;
  received_at: string;
}

export interface StatusResponse {
  announcer_source_generation: number;
  finisher_count: number;
  announcer_rows: AnnouncerRow[];
  devices: DeviceRecord[];
  forwarder_streams: ForwarderStreamRecord[];
}

export async function getStatus(): Promise<StatusResponse> {
  return apiFetch<StatusResponse>("/status");
}

export async function approveDevice(
  endpointId: string,
  displayName: string,
  adminUser = "dev-admin",
): Promise<DeviceRecord> {
  const trimmedName = displayName.trim();
  if (!trimmedName) {
    throw new Error("Display name is required");
  }

  return apiFetch<DeviceRecord>("/admin/devices/approve", {
    method: "POST",
    headers: { "Remote-User": adminUser.trim() || "dev-admin" },
    body: JSON.stringify({
      endpoint_id: endpointId,
      display_name: trimmedName,
    }),
  });
}

export async function renameDevice(
  endpointId: string,
  displayName: string,
  adminUser = "dev-admin",
): Promise<DeviceRecord> {
  const trimmedName = displayName.trim();
  if (!trimmedName) {
    throw new Error("Display name is required");
  }

  return apiFetch<DeviceRecord>("/admin/devices/rename", {
    method: "POST",
    headers: { "Remote-User": adminUser.trim() || "dev-admin" },
    body: JSON.stringify({
      endpoint_id: endpointId,
      display_name: trimmedName,
    }),
  });
}
