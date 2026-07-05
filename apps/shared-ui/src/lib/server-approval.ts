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

/** "active" => "ok", anything else => "warn" */
export function approvalBadgeState(state: string | null): 'ok' | 'warn' {
  return state === 'active' ? 'ok' : 'warn';
}
