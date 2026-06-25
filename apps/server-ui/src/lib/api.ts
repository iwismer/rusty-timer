import { apiFetch } from "@rusty-timer/shared-ui/lib/api-helpers";

export type DeviceKind = "forwarder" | "receiver";
export type ApprovalState = "pending" | "active";

export interface DeviceRecord {
  endpoint_id: string;
  device_kind: DeviceKind;
  approval_state: ApprovalState;
  display_name: string | null;
}

export interface ForwarderStreamRecord {
  stream_id: string;
  endpoint_id: string;
  epoch: number;
  next_seq: number;
}

export interface ForwarderRecord {
  endpoint_id: string;
  display_name: string | null;
  direct_addrs: string[];
  last_seen_unix_ms: number;
  approval_state: ApprovalState;
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
  forwarders: ForwarderRecord[];
  forwarder_streams: ForwarderStreamRecord[];
}

export type EnrollmentTokenStatus = "active" | "used" | "revoked";

export interface EnrollmentTokenRecord {
  token_id: string;
  device_kind: DeviceKind;
  display_name: string | null;
  status: EnrollmentTokenStatus;
  created_unix_ms: number;
  used_unix_ms: number | null;
  used_endpoint_id: string | null;
  revoked_unix_ms: number | null;
}

export interface EnrollmentTokensResponse {
  tokens: EnrollmentTokenRecord[];
}

export interface CreateEnrollmentTokenRequest {
  device_kind: DeviceKind;
  display_name?: string;
  token?: string;
}

export interface CreateEnrollmentTokenResponse {
  token_id: string;
  device_kind: DeviceKind;
  display_name: string | null;
  token: string;
  created_unix_ms: number;
}

export async function getStatus(): Promise<StatusResponse> {
  return apiFetch<StatusResponse>("/status");
}

export async function approveDevice(
  endpointId: string,
  adminUser = "dev-admin",
): Promise<DeviceRecord> {
  return apiFetch<DeviceRecord>("/admin/devices/approve", {
    method: "POST",
    headers: { "Remote-User": adminUser.trim() || "dev-admin" },
    body: JSON.stringify({
      endpoint_id: endpointId,
    }),
  });
}

function adminHeaders(adminUser: string) {
  return { "Remote-User": adminUser.trim() || "dev-admin" };
}

export async function listEnrollmentTokens(
  adminUser = "dev-admin",
): Promise<EnrollmentTokensResponse> {
  return apiFetch<EnrollmentTokensResponse>("/admin/enrollment-tokens", {
    headers: adminHeaders(adminUser),
  });
}

export async function createEnrollmentToken(
  req: CreateEnrollmentTokenRequest,
  adminUser = "dev-admin",
): Promise<CreateEnrollmentTokenResponse> {
  return apiFetch<CreateEnrollmentTokenResponse>("/admin/enrollment-tokens", {
    method: "POST",
    headers: adminHeaders(adminUser),
    body: JSON.stringify(req),
  });
}

export async function revokeEnrollmentToken(
  tokenId: string,
  adminUser = "dev-admin",
): Promise<EnrollmentTokenRecord> {
  return apiFetch<EnrollmentTokenRecord>(
    `/admin/enrollment-tokens/${encodeURIComponent(tokenId)}/revoke`,
    {
      method: "POST",
      headers: adminHeaders(adminUser),
    },
  );
}
