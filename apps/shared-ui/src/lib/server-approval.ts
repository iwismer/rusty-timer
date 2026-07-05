export interface ServerApprovalLike {
  reachable: boolean | null;
  waiting_for_approval: boolean;
}

/** warn while waiting for approval, err when unreachable, muted otherwise */
export function serverApprovalTextClass(server: ServerApprovalLike): string {
  if (server.waiting_for_approval) return 'text-status-warn';
  if (server.reachable === false) return 'text-status-err';
  return 'text-text-muted';
}

/**
 * Badge state for DEVICE approval states only: "active" => "ok", anything
 * else (pending, null) => "warn". Do not use for enrollment-token statuses,
 * where "used"/"expired"/"revoked" need their own treatment.
 */
export function approvalBadgeState(state: string | null): 'ok' | 'warn' {
  return state === 'active' ? 'ok' : 'warn';
}
